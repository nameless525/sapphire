# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""The per-vertex and per-face data Sapphire's glTF importer reads (`tools/sapphire_pipeline/src/import/
gltf.rs`), as mesh attributes the export writes:

- `_MATERIAL_ID`: an integer attribute on faces, 0 to 7, the region of its material a face shades with
  when the material has no control map. The exporter spreads it to the face's corners, so a triangle's
  vertices always share it, as the importer requires.
- The data color, `sapphire_data`: a float color attribute (byte colors are converted from sRGB on
  export), R ambient occlusion, G outline width scale, B outline Z offset scale. Without it the importer
  takes 1 for every channel. It exports as `COLOR_0`.
- `_OUTLINE_NORMAL`: a vector attribute on points, the direction the inverted hull extrudes along, in the
  object's space. Blender's exporter writes custom vectors as they are, so the export turns it into the
  space it writes the mesh's normals in (a skinned mesh's armature's world, Y-up) as it writes it
  (`export.outline_normals_in_gltf_space`). Without it the cook bakes one
  by welding the shading normals of each position.

The functions here work on a mesh in object mode and are what scripts call; the operators wrap them for
edit mode's selection. The hull preview draws each mesh's inverted hull with a geometry nodes modifier."""

import bmesh
import bpy
import numpy as np
from bpy.props import BoolProperty, EnumProperty, FloatProperty, IntProperty
from mathutils import Vector
from mathutils.bvhtree import BVHTree
from mathutils.geometry import closest_point_on_tri
from mathutils.interpolate import poly_3d_calc

MATERIAL_ID = "_MATERIAL_ID"
OUTLINE_NORMAL = "_OUTLINE_NORMAL"
DATA_COLOR = "sapphire_data"
MATERIAL_IDS = 8
# The channels of the data color.
CHANNELS = {"AO": 0, "WIDTH": 1, "Z_OFFSET": 2}
# Positions this close weld when the mesh's own outline normals are made: the cook's weld tolerance
# (a mesh sidecar's `outline: (weld_tolerance: 0.0001)`).
WELD = 1e-4

# The hull preview: its modifier, node group, material and the per-face width it reads.
HULL_MODIFIER = "Sapphire hull"
HULL_GROUP = "Sapphire hull preview"
HULL_MATERIAL = "Sapphire hull"
HULL_WIDTH = "sapphire_hull_width"
# A generated material's outline width in pixels when its extras set none (`Outline::default`).
DEFAULT_OUTLINE_WIDTH = 5.5


class MeshDataError(Exception):
    """A mesh whose attribute of one of these names has another type or domain."""


def _attribute(mesh, name, data_type, domain):
    """`mesh`'s attribute `name`, made if it's missing; refused if one of another kind holds the name."""
    found = mesh.attributes.get(name)
    if found is None:
        return mesh.attributes.new(name, data_type, domain)
    if found.data_type != data_type or found.domain != domain:
        raise MeshDataError(
            f"{mesh.name}: {name} is a {found.data_type} attribute on {found.domain}, where Sapphire needs "
            f"{data_type} on {domain}: remove or convert it"
        )
    return found


def assign_material_id(mesh, material_id, faces=None):
    """Sets `_MATERIAL_ID` to `material_id` on `faces` (polygon indices; all of them when None)."""
    if not 0 <= material_id < MATERIAL_IDS:
        raise MeshDataError(f"material ID {material_id} isn't from 0 to {MATERIAL_IDS - 1}")
    layer = _attribute(mesh, MATERIAL_ID, "INT", "FACE")
    values = np.empty(len(mesh.polygons), dtype=np.int32)
    layer.data.foreach_get("value", values)
    if faces is None:
        values[:] = material_id
    else:
        values[np.asarray(list(faces), dtype=np.int64)] = material_id
    layer.data.foreach_set("value", values)
    mesh.update()


def material_ids(mesh):
    """Each polygon's material ID (0 without the attribute)."""
    layer = mesh.attributes.get(MATERIAL_ID)
    values = np.zeros(len(mesh.polygons), dtype=np.int32)
    if layer is not None:
        layer.data.foreach_get("value", values)
    return values


def data_color(mesh):
    """`mesh`'s data color attribute, made on points and filled with ones if it's missing."""
    found = mesh.color_attributes.get(DATA_COLOR)
    if found is not None:
        if found.data_type != "FLOAT_COLOR":
            raise MeshDataError(
                f"{mesh.name}: {DATA_COLOR} holds byte colors, which the exporter converts from sRGB: "
                "convert it to a float color attribute"
            )
        return found
    made = mesh.color_attributes.new(DATA_COLOR, "FLOAT_COLOR", "POINT")
    made.data.foreach_set("color", np.ones(len(made.data) * 4, dtype=np.float32))
    return made


def set_data_channel(mesh, channel, value, vertices=None):
    """Sets one channel of the data color (`AO`, `WIDTH` or `Z_OFFSET`) to `value` on `vertices` (vertex
    indices; all of them when None), on whichever domain the attribute is."""
    if not 0.0 <= value <= 1.0:
        raise MeshDataError(f"{channel} {value}: data color channels run from 0 to 1")
    colors = data_color(mesh)
    texels = np.empty(len(colors.data) * 4, dtype=np.float32)
    colors.data.foreach_get("color", texels)
    texels = texels.reshape(-1, 4)
    chosen = np.ones(len(mesh.vertices), dtype=bool)
    if vertices is not None:
        chosen[:] = False
        chosen[np.asarray(list(vertices), dtype=np.int64)] = True
    if colors.domain == "CORNER":
        corner_vertices = np.empty(len(mesh.loops), dtype=np.int32)
        mesh.loops.foreach_get("vertex_index", corner_vertices)
        chosen = chosen[corner_vertices]
    texels[chosen, CHANNELS[channel]] = value
    colors.data.foreach_set("color", texels.ravel())
    mesh.update()


def welded_normals(mesh):
    """Each vertex's normal averaged over every face around its position, weighted by the face's angle
    there: the vertices at one position (a hard edge's or a UV seam's split, or separate parts that touch)
    all get the same unit normal."""
    mesh.calc_loop_triangles()
    count = len(mesh.vertices)
    positions = np.empty(count * 3, dtype=np.float64)
    mesh.vertices.foreach_get("co", positions)
    positions = positions.reshape(-1, 3)
    triangles = np.empty(len(mesh.loop_triangles) * 3, dtype=np.int64)
    mesh.loop_triangles.foreach_get("vertices", triangles)
    triangles = triangles.reshape(-1, 3)
    # Summed in an order of their vertices, never the faces': a mesh's face order can vary between runs
    # (bmesh fills deleted faces' slots in a varying order), and floating-point sums follow their order.
    by_vertices = np.sort(triangles, axis=1)
    triangles = triangles[np.lexsort((by_vertices[:, 2], by_vertices[:, 1], by_vertices[:, 0]))]
    corners = positions[triangles]
    face_normals = np.cross(corners[:, 1] - corners[:, 0], corners[:, 2] - corners[:, 0])
    lengths = np.linalg.norm(face_normals, axis=1, keepdims=True)
    face_normals = face_normals / np.maximum(lengths, 1e-30)
    sums = np.zeros((count, 3))
    for corner in range(3):
        to_next = corners[:, (corner + 1) % 3] - corners[:, corner]
        to_previous = corners[:, (corner + 2) % 3] - corners[:, corner]
        cosine = np.einsum("ij,ij->i", to_next, to_previous) / np.maximum(
            np.linalg.norm(to_next, axis=1) * np.linalg.norm(to_previous, axis=1), 1e-30
        )
        angle = np.arccos(np.clip(cosine, -1.0, 1.0))
        np.add.at(sums, triangles[:, corner], face_normals * angle[:, None])
    # Weld: one key per position, at the cook's tolerance.
    keys = np.round(positions / WELD).astype(np.int64)
    _, group = np.unique(keys, axis=0, return_inverse=True)
    group = group.ravel()
    welded = np.zeros((group.max() + 1 if count else 0, 3))
    np.add.at(welded, group, sums)
    normals = welded[group]
    lengths = np.linalg.norm(normals, axis=1, keepdims=True)
    return np.where(lengths > 1e-12, normals / np.maximum(lengths, 1e-30), [0.0, 0.0, 1.0])


def _nearest_on_polygon(point, corners):
    """The squared distance from `point` to a polygon (as a fan of triangles from its first corner) and
    its nearest point there, the first triangle's on a tie."""
    best = None
    for k in range(1, len(corners) - 1):
        on_triangle = closest_point_on_tri(point, corners[0], corners[k], corners[k + 1])
        distance = (on_triangle - point).length_squared
        if best is None or distance < best[0]:
            best = (distance, on_triangle)
    return best


def transferred_normals(obj, source, depsgraph):
    """Each of `obj`'s vertices' normal taken from the nearest point of `source`'s surface, interpolated
    from its smooth vertex normals, in `obj`'s space."""
    evaluated = source.evaluated_get(depsgraph)
    source_mesh = evaluated.to_mesh()
    try:
        to_world = evaluated.matrix_world
        normal_to_world = to_world.to_3x3().inverted_safe().transposed()
        bm = bmesh.new()
        bm.from_mesh(source_mesh)
        bm.transform(to_world)
        tree = BVHTree.FromBMesh(bm)
        bm.free()
        polygons = source_mesh.polygons
        source_positions = [to_world @ vertex.co for vertex in source_mesh.vertices]
        # Smooth normals made here rather than Blender's, which it sums on several threads, so their last
        # bits vary between runs.
        source_normals = [(normal_to_world @ Vector(normal)).normalized() for normal in welded_normals(source_mesh)]
        local = obj.matrix_world
        # A world normal back into `obj`'s space: by the transpose of its linear part.
        normal_to_local = local.to_3x3().transposed()
        normals = np.empty((len(obj.data.vertices), 3))
        for index, vertex in enumerate(obj.data.vertices):
            point = local @ vertex.co
            found = tree.find_nearest(point)
            if found[0] is None:
                raise MeshDataError(f"{source.name} has no faces to transfer normals from")
            # Where faces tie (a point nearest an edge or a corner they share) the tree's answer follows its
            # build, and the source's face order can vary between runs: the faces within reach are measured
            # again here, and the nearest wins, then the one with the lowest vertices, so a transfer repeats
            # bit for bit.
            candidates = {candidate[2] for candidate in tree.find_nearest_range(point, found[3] + 1e-6)}
            best = None
            for face in candidates or {found[2]}:
                distance, on_face = _nearest_on_polygon(point, [source_positions[c] for c in polygons[face].vertices])
                key = (distance, sorted(polygons[face].vertices))
                if best is None or key < best[0]:
                    best = (key, face, on_face)
            _, face, nearest = best
            corners = polygons[face].vertices
            weights = poly_3d_calc([source_positions[corner] for corner in corners], nearest)
            normal = Vector()
            for corner, weight in zip(corners, weights):
                normal += source_normals[corner] * weight
            normals[index] = (normal_to_local @ normal).normalized()
        return normals
    finally:
        evaluated.to_mesh_clear()


def set_outline_normals(obj, source=None, depsgraph=None):
    """Writes `_OUTLINE_NORMAL` on `obj`'s mesh: transferred from `source`'s smooth normals, or with none,
    the mesh's own welded normals (what the cook would bake, made visible and editable)."""
    mesh = obj.data
    if source is None:
        normals = welded_normals(mesh)
    else:
        normals = transferred_normals(obj, source, depsgraph or bpy.context.evaluated_depsgraph_get())
    layer = _attribute(mesh, OUTLINE_NORMAL, "FLOAT_VECTOR", "POINT")
    layer.data.foreach_set("vector", normals.astype(np.float32).ravel())
    mesh.update()


def clear_outline_normals(mesh):
    """Removes `_OUTLINE_NORMAL`: the cook bakes welded ones again."""
    found = mesh.attributes.get(OUTLINE_NORMAL)
    if found is not None:
        mesh.attributes.remove(found)
        mesh.update()


def outline_width(material):
    """The outline width in pixels a material's extras give its generated material (`character_shading`'s
    fourth number), or the engine's default."""
    if material is not None and "character_shading" in material:
        return float(material["character_shading"][3])
    return DEFAULT_OUTLINE_WIDTH


def _hull_material():
    material = bpy.data.materials.get(HULL_MATERIAL)
    if material is None:
        material = bpy.data.materials.new(HULL_MATERIAL)
        material.diffuse_color = (0.02, 0.015, 0.025, 1.0)
        material.use_backface_culling = True
        principled = material.node_tree.nodes.get("Principled BSDF")
        if principled is not None:
            principled.inputs["Base Color"].default_value = (0.02, 0.015, 0.025, 1.0)
    return material


def _hull_group():
    """The preview's node group: the mesh, joined with a copy pushed out along the outline normal (the
    shading normal without one) by width × G × pixel, faces flipped, in the hull material."""
    group = bpy.data.node_groups.get(HULL_GROUP)
    if group is not None:
        return group
    group = bpy.data.node_groups.new(HULL_GROUP, "GeometryNodeTree")
    group.interface.new_socket("Geometry", in_out="INPUT", socket_type="NodeSocketGeometry")
    pixel_socket = group.interface.new_socket("Pixel", in_out="INPUT", socket_type="NodeSocketFloat")
    pixel_socket.default_value = 0.0023
    group.interface.new_socket("Geometry", in_out="OUTPUT", socket_type="NodeSocketGeometry")
    nodes, links = group.nodes, group.links
    inputs = nodes.new("NodeGroupInput")
    outputs = nodes.new("NodeGroupOutput")

    outline = nodes.new("GeometryNodeInputNamedAttribute")
    outline.data_type = "FLOAT_VECTOR"
    outline.inputs["Name"].default_value = OUTLINE_NORMAL
    shading = nodes.new("GeometryNodeInputNormal")
    direction = nodes.new("GeometryNodeSwitch")
    direction.input_type = "VECTOR"
    links.new(outline.outputs["Exists"], direction.inputs["Switch"])
    links.new(shading.outputs["Normal"], direction.inputs["False"])
    links.new(outline.outputs["Attribute"], direction.inputs["True"])
    unit = nodes.new("ShaderNodeVectorMath")
    unit.operation = "NORMALIZE"
    links.new(direction.outputs["Output"], unit.inputs[0])

    color = nodes.new("GeometryNodeInputNamedAttribute")
    color.data_type = "FLOAT_COLOR"
    color.inputs["Name"].default_value = DATA_COLOR
    channels = nodes.new("FunctionNodeSeparateColor")
    links.new(color.outputs["Attribute"], channels.inputs["Color"])
    green = nodes.new("GeometryNodeSwitch")
    green.input_type = "FLOAT"
    green.inputs["False"].default_value = 1.0
    links.new(color.outputs["Exists"], green.inputs["Switch"])
    links.new(channels.outputs["Green"], green.inputs["True"])

    width = nodes.new("GeometryNodeInputNamedAttribute")
    width.data_type = "FLOAT"
    width.inputs["Name"].default_value = HULL_WIDTH
    scaled = nodes.new("ShaderNodeMath")
    scaled.operation = "MULTIPLY"
    links.new(width.outputs["Attribute"], scaled.inputs[0])
    links.new(green.outputs["Output"], scaled.inputs[1])
    meters = nodes.new("ShaderNodeMath")
    meters.operation = "MULTIPLY"
    links.new(scaled.outputs[0], meters.inputs[0])
    links.new(inputs.outputs["Pixel"], meters.inputs[1])
    offset = nodes.new("ShaderNodeVectorMath")
    offset.operation = "SCALE"
    links.new(unit.outputs["Vector"], offset.inputs[0])
    links.new(meters.outputs[0], offset.inputs["Scale"])

    moved = nodes.new("GeometryNodeSetPosition")
    links.new(inputs.outputs["Geometry"], moved.inputs["Geometry"])
    links.new(offset.outputs["Vector"], moved.inputs["Offset"])
    flipped = nodes.new("GeometryNodeFlipFaces")
    links.new(moved.outputs["Geometry"], flipped.inputs["Mesh"])
    painted = nodes.new("GeometryNodeSetMaterial")
    painted.inputs["Material"].default_value = _hull_material()
    links.new(flipped.outputs["Mesh"], painted.inputs["Geometry"])
    joined = nodes.new("GeometryNodeJoinGeometry")
    links.new(painted.outputs["Geometry"], joined.inputs["Geometry"])
    links.new(inputs.outputs["Geometry"], joined.inputs["Geometry"])
    links.new(joined.outputs["Geometry"], outputs.inputs["Geometry"])
    return group


def hull_modifier(obj):
    """`obj`'s hull preview modifier, or None."""
    return next((modifier for modifier in obj.modifiers if modifier.name == HULL_MODIFIER), None)


def show_hull(obj, pixel):
    """Adds (or refreshes) `obj`'s hull preview: each face's width from its material's extras, times the
    data color's G, at `pixel` meters a pixel."""
    mesh = obj.data
    widths = np.array([outline_width(material) for material in mesh.materials] or [DEFAULT_OUTLINE_WIDTH])
    slots = np.empty(len(mesh.polygons), dtype=np.int32)
    mesh.polygons.foreach_get("material_index", slots)
    layer = mesh.attributes.get(HULL_WIDTH) or mesh.attributes.new(HULL_WIDTH, "FLOAT", "FACE")
    layer.data.foreach_set("value", widths[np.clip(slots, 0, len(widths) - 1)].astype(np.float32))
    modifier = hull_modifier(obj)
    if modifier is None:
        modifier = obj.modifiers.new(HULL_MODIFIER, "NODES")
        modifier.node_group = _hull_group()
    pixel_input = modifier.node_group.interface.items_tree["Pixel"].identifier
    getattr(modifier.properties.inputs, pixel_input).value = pixel
    mesh.update()


def hide_hull(obj):
    """Removes `obj`'s hull preview and the widths it read."""
    modifier = hull_modifier(obj)
    if modifier is not None:
        obj.modifiers.remove(modifier)
    layer = obj.data.attributes.get(HULL_WIDTH)
    if layer is not None:
        obj.data.attributes.remove(layer)


class _EditSelection:
    """In edit mode, leaves it for the operator's change and returns after, so the mesh's selection and
    data are current; outside edit mode does nothing."""

    def __init__(self, context):
        self.edit = context.mode == "EDIT_MESH"

    def __enter__(self):
        if self.edit:
            bpy.ops.object.mode_set(mode="OBJECT")
        return self

    def __exit__(self, *_):
        if self.edit:
            bpy.ops.object.mode_set(mode="EDIT")

    def faces(self, mesh):
        return [polygon.index for polygon in mesh.polygons if polygon.select] if self.edit else None

    def vertices(self, mesh):
        return [vertex.index for vertex in mesh.vertices if vertex.select] if self.edit else None


def _mesh_poll(context):
    return context.active_object is not None and context.active_object.type == "MESH"


class SAPPHIRE_OT_material_id_assign(bpy.types.Operator):
    """Paint the material ID on the selected faces (in object mode, on every face)"""

    bl_idname = "sapphire.material_id_assign"
    bl_label = "Assign Material ID"
    bl_options = {"REGISTER", "UNDO"}

    material_id: IntProperty(name="ID", min=0, max=MATERIAL_IDS - 1)

    @classmethod
    def poll(cls, context):
        return _mesh_poll(context)

    def execute(self, context):
        mesh = context.active_object.data
        try:
            with _EditSelection(context) as selection:
                assign_material_id(mesh, self.material_id, selection.faces(mesh))
        except MeshDataError as error:
            self.report({"ERROR"}, str(error))
            return {"CANCELLED"}
        return {"FINISHED"}


class SAPPHIRE_OT_material_id_select(bpy.types.Operator):
    """Select the faces with this material ID"""

    bl_idname = "sapphire.material_id_select"
    bl_label = "Select Material ID"
    bl_options = {"REGISTER", "UNDO"}

    material_id: IntProperty(name="ID", min=0, max=MATERIAL_IDS - 1)

    @classmethod
    def poll(cls, context):
        return _mesh_poll(context) and context.mode == "EDIT_MESH"

    def execute(self, context):
        mesh = context.active_object.data
        with _EditSelection(context):
            chosen = material_ids(mesh) == self.material_id
            mesh.polygons.foreach_set("select", chosen)
        return {"FINISHED"}


class SAPPHIRE_OT_data_color_set(bpy.types.Operator):
    """Set a channel of the data color on the selected vertices (in object mode, on every vertex)"""

    bl_idname = "sapphire.data_color_set"
    bl_label = "Set Data Color"
    bl_options = {"REGISTER", "UNDO"}

    channel: EnumProperty(
        name="Channel",
        items=(
            ("AO", "Ambient occlusion", "R: darkens the shadow bias, 1 unoccluded"),
            ("WIDTH", "Outline width", "G: scales the outline's width, 0 hides it"),
            ("Z_OFFSET", "Outline Z offset", "B: scales how far the hull is pushed from the camera"),
        ),
    )
    value: FloatProperty(name="Value", min=0.0, max=1.0, default=1.0)

    @classmethod
    def poll(cls, context):
        return _mesh_poll(context)

    def execute(self, context):
        mesh = context.active_object.data
        try:
            with _EditSelection(context) as selection:
                set_data_channel(mesh, self.channel, self.value, selection.vertices(mesh))
        except MeshDataError as error:
            self.report({"ERROR"}, str(error))
            return {"CANCELLED"}
        return {"FINISHED"}


class SAPPHIRE_OT_outline_normals(bpy.types.Operator):
    """Write the outline normals the hull extrudes along: transferred from the source mesh's smooth
    normals, or the mesh's own welded normals with no source"""

    bl_idname = "sapphire.outline_normals"
    bl_label = "Transfer Outline Normals"
    bl_options = {"REGISTER", "UNDO"}

    @classmethod
    def poll(cls, context):
        return _mesh_poll(context)

    def execute(self, context):
        obj = context.active_object
        source = context.scene.sapphire.outline_source
        if source is obj:
            self.report({"ERROR"}, "the source is the mesh itself: clear it to weld the mesh's own normals")
            return {"CANCELLED"}
        try:
            with _EditSelection(context):
                set_outline_normals(obj, source, context.evaluated_depsgraph_get())
        except MeshDataError as error:
            self.report({"ERROR"}, str(error))
            return {"CANCELLED"}
        return {"FINISHED"}


class SAPPHIRE_OT_outline_normals_clear(bpy.types.Operator):
    """Remove the outline normals: the cook welds the shading normals instead"""

    bl_idname = "sapphire.outline_normals_clear"
    bl_label = "Clear Outline Normals"
    bl_options = {"REGISTER", "UNDO"}

    @classmethod
    def poll(cls, context):
        return _mesh_poll(context) and OUTLINE_NORMAL in context.active_object.data.attributes

    def execute(self, context):
        with _EditSelection(context):
            clear_outline_normals(context.active_object.data)
        return {"FINISHED"}


class SAPPHIRE_OT_hull_preview(bpy.types.Operator):
    """Show the inverted hull the engine draws, at its width in pixels at the outline curve's mid
    distance; again to refresh its widths, or hide it"""

    bl_idname = "sapphire.hull_preview"
    bl_label = "Hull Preview"
    bl_options = {"REGISTER", "UNDO"}

    show: BoolProperty(name="Show", default=True)

    @classmethod
    def poll(cls, context):
        return _mesh_poll(context)

    def execute(self, context):
        obj = context.active_object
        with _EditSelection(context):
            if self.show:
                show_hull(obj, context.scene.sapphire.hull_pixel)
            else:
                hide_hull(obj)
        return {"FINISHED"}


CLASSES = (
    SAPPHIRE_OT_material_id_assign,
    SAPPHIRE_OT_material_id_select,
    SAPPHIRE_OT_data_color_set,
    SAPPHIRE_OT_outline_normals,
    SAPPHIRE_OT_outline_normals_clear,
    SAPPHIRE_OT_hull_preview,
)

register, unregister = bpy.utils.register_classes_factory(CLASSES)
