"""Builds the FBX importer's test scenes and exports each as both FBX and GLB (M1 A10).

Run it headless; it starts from an empty scene, saves no .blend and writes only the fixtures:

    blender -b --factory-startup --python tools/blender/fbx_fixtures.py -- \
        --out tools/sapphire_pipeline/fixtures/fbx

The pipeline's tests import each scene from both files and require the same mesh, so both
exports must describe one scene in the same terms. The scenes are made here from numbers, so they
are the project's own (MIT):

- **static_multi**: three objects in a hierarchy, with rotations, a non-uniform scale on a leaf
  and a mirrored copy (negative scale, so its winding flips), in three materials (one shared by
  two objects), smooth-shaded with sharp edges split, one UV map and one float color attribute
  holding data in every channel.
- **skinned_morph**: a tube skinned to a chain of three bones under an armature, up to three
  influences a vertex, in two materials, with three shape keys, and a cap parented to the last
  bone, which the importers bind rigidly to it. Blender's FBX exporter writes normals for the mesh
  as its shape keys currently mix, where its glTF exporter resets them first, so the only key
  with a weight (0.25, "Rise") moves every vertex alike, which turns no normal.
- **textured** (FBX only, as `textured_external.fbx` naming `textures/checker.png` and
  `textures/bumps.png` beside it, and `textured_embedded.fbx` embedding them): a box whose
  material samples a base color texture and a normal map, for `extract-materials`.

Every face is a triangle, so neither exporter triangulates: the FBX importer's own triangulation
is tested elsewhere, on the pavilion and on hand-written polygons. The exports agree in what they
write:

- FBX colors as stored (`colors_type='LINEAR'`): the importers read vertex colors as data, and
  Blender's FBX default would convert the float attribute to sRGB.
- glTF with the active color attribute (`export_vertex_color='ACTIVE'`; by default it writes
  only colors a material reads), through a workaround for Blender 5.2's exporter, which writes
  them for a mesh's first material only (`keep_every_materials_vertex_colors`).
- FBX without leaf bones and animation; glTF without animation.

Every run writes the same bytes, wherever the checkout is (`reproducible_fbx`): the FBX exporter's
time stamp is fixed, its object IDs hash their keys without Python's per-process salt, and images
are named by their path relative to the FBX alone, where the exporter also writes the absolute
path, which would put the machine's directories in the files.
"""

import argparse
import datetime
import hashlib
import math
import os
import sys

import bpy

# Colors as linear data: R ambient occlusion, G outline width, B outline depth offset.
MATERIALS = {
    "Lacquer": (0.62, 0.08, 0.04, 1.0),
    "Stone": (0.42, 0.40, 0.37, 1.0),
    "Gold": (0.80, 0.56, 0.12, 1.0),
    "Skin": (0.90, 0.72, 0.62, 1.0),
    "Cloth": (0.18, 0.22, 0.45, 1.0),
}


def parse_arguments():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(prog="fbx_fixtures.py", description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, help="directory for the .fbx and .glb fixtures")
    return parser.parse_args(argv)


def empty_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0
    return scene


def material(name):
    found = bpy.data.materials.get(name)
    if found is not None:
        return found
    created = bpy.data.materials.new(name)
    created.diffuse_color = MATERIALS[name]
    # The base color both exporters write: Principled BSDF's (new materials have one), which FBX
    # keeps as DiffuseColor and glTF as baseColorFactor.
    principled = created.node_tree.nodes.get("Principled BSDF")
    principled.inputs["Base Color"].default_value = MATERIALS[name]
    principled.inputs["Roughness"].default_value = 0.6
    return created


def mesh_object(name, vertices, triangles, materials, face_materials, collection):
    """A mesh object of `triangles` (vertex index triples), smooth-shaded with edges sharper than
    50° split (an octagon's sides are smooth, a hexagon's hard), a UV map and a float color
    attribute, both per face corner."""
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(vertices, [], triangles)
    for slot in materials:
        mesh.materials.append(material(slot))
    for polygon, slot in zip(mesh.polygons, face_materials):
        polygon.material_index = slot
    mesh.shade_smooth()
    mesh.set_sharp_from_angle(angle=math.radians(50.0))
    uv = mesh.uv_layers.new(name="UVMap")
    colors = mesh.color_attributes.new(name="Data", type="FLOAT_COLOR", domain="CORNER")
    for polygon in mesh.polygons:
        for corner in polygon.loop_indices:
            x, y, z = mesh.vertices[mesh.loops[corner].vertex_index].co
            # Coordinates from the position and the face, so corners of one vertex can differ.
            uv.data[corner].uv = (0.37 * x + 0.21 * z + 0.5, 0.29 * y - 0.17 * z + 0.25 + 0.05 * polygon.index % 0.4)
            colors.data[corner].color = (
                0.25 + 0.5 * ((0.31 * x + 0.13 * polygon.index) % 1.0),
                0.5 + 0.4 * math.sin(1.7 * y + 0.3 * z),
                (0.11 * polygon.index + 0.2 * z) % 1.0,
                1.0,
            )
    mesh.color_attributes.active_color = colors
    mesh.validate()
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    return obj


def prism(sides, radius, height, base=0.0):
    """A closed prism about +Z, as triangles: the sides as quads split corner to corner, the caps
    as fans."""
    vertices = []
    for ring in range(2):
        for side in range(sides):
            angle = 2.0 * math.pi * side / sides
            vertices.append((radius * math.cos(angle), radius * math.sin(angle), base + ring * height))
    bottom_center = len(vertices)
    vertices.append((0.0, 0.0, base))
    top_center = len(vertices)
    vertices.append((0.0, 0.0, base + height))
    triangles, faces = [], []
    for side in range(sides):
        a, b = side, (side + 1) % sides
        triangles += [(a, b, sides + b), (a, sides + b, sides + a)]
        faces += ["side", "side"]
        triangles.append((bottom_center, b, a))
        faces.append("bottom")
        triangles.append((top_center, sides + a, sides + b))
        faces.append("top")
    return vertices, triangles, faces


def static_multi():
    """Three objects in three materials, one shared; see the module docs."""
    collection = empty_scene().collection
    vertices, triangles, faces = prism(8, 0.4, 2.0)
    slots = {"side": 0, "bottom": 1, "top": 2}
    pillar = mesh_object(
        "Pillar", vertices, triangles, ["Lacquer", "Stone", "Gold"], [slots[f] for f in faces], collection
    )
    pillar.location = (1.0, 2.0, 0.5)
    pillar.rotation_euler = (math.radians(20.0), 0.0, math.radians(35.0))

    vertices, triangles, faces = prism(6, 0.55, 0.3)
    cap = mesh_object("Cap", vertices, triangles, ["Gold"], [0] * len(triangles), collection)
    cap.parent = pillar
    cap.location = (0.0, 0.0, 2.0)
    cap.rotation_euler = (0.0, math.radians(10.0), math.radians(15.0))
    cap.scale = (1.0, 1.4, 0.8)

    vertices, triangles, faces = prism(5, 0.3, 1.2)
    mirror = mesh_object(
        "Mirror", vertices, triangles, ["Stone", "Gold"], [1 if f == "top" else 0 for f in faces], collection
    )
    mirror.location = (-1.5, 0.5, 0.0)
    mirror.rotation_euler = (0.0, 0.0, math.radians(-25.0))
    mirror.scale = (-1.0, 1.0, 1.0)


def skinned_morph():
    """A tube on three bones with three shape keys, and a cap on the last bone; see the module docs."""
    collection = empty_scene().collection
    armature_data = bpy.data.armatures.new("Armature")
    armature = bpy.data.objects.new("Armature", armature_data)
    collection.objects.link(armature)
    bpy.context.view_layer.objects.active = armature
    bpy.ops.object.mode_set(mode="EDIT")
    heads = [(0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (0.0, 0.1, 2.0)]
    tails = [(0.0, 0.0, 1.0), (0.0, 0.1, 2.0), (0.0, 0.35, 3.0)]
    parent = None
    for name, head, tail in zip(("Root", "Upper", "Tip"), heads, tails):
        bone = armature_data.edit_bones.new(name)
        bone.head, bone.tail = head, tail
        bone.roll = math.radians(12.0) if name == "Upper" else 0.0
        if parent is not None:
            bone.parent = parent
            bone.use_connect = True
        parent = bone
    bpy.ops.object.mode_set(mode="OBJECT")

    # A tube of 8 sides and 7 rings from z = 0 to 3, capped.
    sides, rings = 8, 7
    vertices = []
    for ring in range(rings):
        z = 3.0 * ring / (rings - 1)
        for side in range(sides):
            angle = 2.0 * math.pi * side / sides
            vertices.append((0.3 * math.cos(angle), 0.3 * math.sin(angle) + 0.1 * z * z / 9.0, z))
    triangles, slots = [], []
    for ring in range(rings - 1):
        for side in range(sides):
            a, b = ring * sides + side, ring * sides + (side + 1) % sides
            triangles += [(a, b, b + sides), (a, b + sides, a + sides)]
            # Cloth below the middle, skin above.
            slots += [0 if ring < 3 else 1] * 2
    bottom, top = len(vertices), len(vertices) + 1
    vertices += [(0.0, 0.0, 0.0), (0.0, 0.1, 3.0)]
    for side in range(sides):
        a, b = side, (side + 1) % sides
        triangles.append((bottom, b, a))
        slots.append(0)
        last = (rings - 1) * sides
        triangles.append((top, last + a, last + b))
        slots.append(1)
    body = mesh_object("Body", vertices, triangles, ["Cloth", "Skin"], slots, collection)
    body.parent = armature
    modifier = body.modifiers.new("Armature", "ARMATURE")
    modifier.object = armature

    # Up to three influences a vertex, falling off with distance from each bone's middle.
    groups = {name: body.vertex_groups.new(name=name) for name in ("Root", "Upper", "Tip")}
    for index, (_, _, z) in enumerate(vertices):
        raw = {name: max(0.0, 1.0 - abs(z - middle) / 1.4) for name, middle in (("Root", 0.5), ("Upper", 1.5), ("Tip", 2.5))}
        total = sum(raw.values())
        for name, weight in raw.items():
            if weight > 0.0:
                groups[name].add([index], weight / total, "REPLACE")

    body.shape_key_add(name="Basis", from_mix=False)
    bulge = body.shape_key_add(name="Bulge", from_mix=False)
    for index, (x, y, z) in enumerate(vertices):
        swell = max(0.0, 1.0 - abs(z - 1.5) / 0.8)
        bulge.data[index].co = (x * (1.0 + 0.4 * swell), y * (1.0 + 0.4 * swell), z)
    bulge.value = 0.0
    lean = body.shape_key_add(name="Lean", from_mix=False)
    for index, (x, y, z) in enumerate(vertices):
        lean.data[index].co = (x + 0.15 * z * z / 9.0, y, z)
    lean.value = 0.0
    rise = body.shape_key_add(name="Rise", from_mix=False)
    for index, (x, y, z) in enumerate(vertices):
        rise.data[index].co = (x, y, z + 0.1)
    rise.value = 0.25

    vertices, triangles, _ = prism(6, 0.2, 0.25)
    cap = mesh_object("Cap", vertices, triangles, ["Skin"], [0] * len(triangles), collection)
    cap.parent = armature
    cap.parent_type = "BONE"
    cap.parent_bone = "Tip"
    # Bone parents place children at the bone's tail.
    cap.location = (0.0, 0.0, 0.0)
    cap.rotation_euler = (math.radians(-90.0), 0.0, 0.0)


def image(name, out, pixel):
    """A 4 × 4 PNG made from `pixel(x, y)` (RGBA bytes), saved as `<out>/textures/<name>.png`."""
    created = bpy.data.images.new(name, 4, 4, alpha=True)
    created.pixels = [channel / 255.0 for y in range(4) for x in range(4) for channel in pixel(x, y)]
    created.filepath_raw = os.path.join(out, "textures", f"{name}.png")
    created.file_format = "PNG"
    created.colorspace_settings.name = "sRGB" if name == "checker" else "Non-Color"
    os.makedirs(os.path.dirname(created.filepath_raw), exist_ok=True)
    created.save()
    return created


def textured(out):
    """A prism whose material samples a base color texture and a normal map; see `export_textured`."""
    collection = empty_scene().collection
    vertices, triangles, _ = prism(4, 0.5, 1.0)
    obj = mesh_object("Crate", vertices, triangles, ["Stone"], [0] * len(triangles), collection)
    stone = obj.data.materials[0]
    nodes, links = stone.node_tree.nodes, stone.node_tree.links
    principled = nodes.get("Principled BSDF")
    base = nodes.new("ShaderNodeTexImage")
    base.image = image("checker", out, lambda x, y: (230, 200, 120, 255) if (x + y) % 2 else (60, 40, 30, 255))
    links.new(base.outputs["Color"], principled.inputs["Base Color"])
    normal = nodes.new("ShaderNodeTexImage")
    normal.image = image("bumps", out, lambda x, y: (128 + 20 * (x - 2), 128 + 20 * (y - 2), 255, 255))
    normal_map = nodes.new("ShaderNodeNormalMap")
    links.new(normal.outputs["Color"], normal_map.inputs["Color"])
    links.new(normal_map.outputs["Normal"], principled.inputs["Normal"])


def export_textured(out):
    """The textured scene as an FBX naming its images relative to itself (beside them in
    `textures/`), and as one embedding them."""
    bpy.ops.export_scene.fbx(
        filepath=os.path.join(out, "textured_external.fbx"),
        object_types={"MESH"},
        colors_type="LINEAR",
        bake_anim=False,
        use_metadata=False,
        path_mode="RELATIVE",
    )
    bpy.ops.export_scene.fbx(
        filepath=os.path.join(out, "textured_embedded.fbx"),
        object_types={"MESH"},
        colors_type="LINEAR",
        bake_anim=False,
        use_metadata=False,
        path_mode="COPY",
        embed_textures=True,
    )
    print(f"wrote the textured scene to {out}")


def keep_every_materials_vertex_colors():
    """Works around Blender 5.2's glTF exporter, which writes a mesh's vertex colors into its first
    material's primitive only: for every later material it records the color attribute's name
    where the glTF attribute's (`COLOR_0`) belongs, and then writes those corners white. Puts the
    glTF attribute back before the colors are written; a no-op once the exporter changes."""
    from io_scene_gltf2.blender.exp import primitive_extract

    creator = primitive_extract.PrimitiveCreator
    write_colors = getattr(creator, "_PrimitiveCreator__manage_color_attributes", None)
    if write_colors is None:
        return

    def fixed(self):
        by_key = {
            (info.get("color") or "") + (info.get("alpha") or ""): info["gltf_name"]
            for info in self.vc_infos
            if not info["forced"]
        }
        for material, used in list(self.material_idxs_using_vc.items()):
            self.material_idxs_using_vc[material] = by_key.get(used, used)
        write_colors(self)

    creator._PrimitiveCreator__manage_color_attributes = fixed


def reproducible_fbx():
    """Makes the FBX exporter's output depend on the scene alone: a fixed creation time instead of
    the time of export; object IDs from a stable hash of their keys, where the exporter uses
    Python's `hash`, salted per process; and each image's path relative to the FBX where it writes
    the absolute one too (`FileName`, `Filename` and `Path`). Importers read the relative path
    first."""
    from io_scene_fbx import export_fbx_bin, fbx_utils

    def stable_hash(key):
        digest = hashlib.blake2b(repr(key).encode(), digest_size=8).digest()
        return int.from_bytes(digest, "little", signed=True)

    # `fbx_utils._key_to_uuid` calls `hash` by name: a module global shadows the builtin.
    fbx_utils.hash = stable_hash

    header = export_fbx_bin.fbx_header_elements
    paths = export_fbx_bin._gen_vid_path

    def fixed_time(root, scene_data, time=None):
        return header(root, scene_data, datetime.datetime(2026, 1, 1))

    def relative_only(image, scene_data):
        _, relative = paths(image, scene_data)
        return relative, relative

    export_fbx_bin.fbx_header_elements = fixed_time
    export_fbx_bin._gen_vid_path = relative_only


def export(out, name):
    fbx = os.path.join(out, f"{name}.fbx")
    glb = os.path.join(out, f"{name}.glb")
    bpy.ops.export_scene.fbx(
        filepath=fbx,
        object_types={"ARMATURE", "MESH"},
        colors_type="LINEAR",
        add_leaf_bones=False,
        bake_anim=False,
        use_metadata=False,
    )
    bpy.ops.export_scene.gltf(
        filepath=glb,
        export_format="GLB",
        export_vertex_color="ACTIVE",
        export_animations=False,
        export_materials="EXPORT",
    )
    print(f"wrote {fbx} and {glb}")


def main():
    arguments = parse_arguments()
    os.makedirs(arguments.out, exist_ok=True)
    keep_every_materials_vertex_colors()
    reproducible_fbx()
    for name, build in (("static_multi", static_multi), ("skinned_morph", skinned_morph)):
        build()
        export(arguments.out, name)
    textured(arguments.out)
    export_textured(arguments.out)


if __name__ == "__main__":
    main()
