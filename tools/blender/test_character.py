# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""Builds the project's test character with the add-on and exports it through the add-on's export (M6 C3).

Run it headless; it starts from an empty scene, saves no .blend and writes only the export:

    blender -b --factory-startup --python tools/blender/test_character.py -- \
        --out tools/sapphire_pipeline/fixtures/character

It enables the add-on installed in Blender if there is one (`--installed` requires it), else imports it
from this checkout. With `--check`, it then runs the add-on's Check on the folder (the pipeline binary from
`--pipeline` or `$SAPPHIRE_PIPELINE`); with `--bake <dir>`, it bakes the face's masks into that folder and
draws their sheet; with `--hull`, it shows the body's hull preview and confirms it doubles the body.

The character is made here from numbers, so it is the project's own (MIT): a figure about 1.6 m tall, facing
-Y, on a 17-joint armature, in four objects and four materials, each set with what the importer reads:

- **Body** (Skin and Cloth): a torso, a skirt, a neck, arms and legs, open tubes blended across their bones.
  The skirt's hem band is material ID 1 and its hem ring's data color is (153, 204, 0) / 255; every other
  body vertex is (255, 255, 0) / 255. Its outline normals are its own, welded.
- **Head** (Skin) and **Face** (Face): the back and front halves of a sphere, rigid to `head`, outline
  normals transferred from a sphere on them. The face's UVs project it from the front, mirrored about
  u = 0.5, for the mask bake; its data color is (255, 128, 64) / 255, the head's (255, 255, 0) / 255; it
  has one shape key, `smile`, raising the mouth by 5 mm.
- **Hair** (Hair): a faceted cap, rigid to `head`, its band below 1.58 m material ID 2, data color
  (230, 255, 102) / 255, outline normals transferred from an ellipsoid inside it, so they differ from its
  flat shading normals.
- Each material sets its base color, back-face culling and extras (`MATERIALS`).

Every face is a triangle, split the same way each run, so the exporter never picks a diagonal.

The pipeline's test (`tools/sapphire_pipeline/tests/blender_character.rs`) imports the export and holds
every one of these. Data colors are whole bytes over 255, so they reach the cooked bytes exactly.
"""

import argparse
import math
import os
import sys

import bmesh
import bpy

SCRIPTS = os.path.dirname(os.path.abspath(__file__))

# Bones: name, parent, head, tail.
BONES = (
    ("hips", None, (0.0, 0.0, 0.95), (0.0, 0.0, 1.05)),
    ("spine", "hips", (0.0, 0.0, 1.05), (0.0, 0.0, 1.2)),
    ("chest", "spine", (0.0, 0.0, 1.2), (0.0, 0.0, 1.38)),
    ("neck", "chest", (0.0, 0.0, 1.38), (0.0, 0.0, 1.46)),
    ("head", "neck", (0.0, 0.0, 1.46), (0.0, 0.0, 1.68)),
    ("upperarm.l", "chest", (0.17, 0.0, 1.34), (0.42, 0.0, 1.34)),
    ("lowerarm.l", "upperarm.l", (0.42, 0.0, 1.34), (0.66, 0.0, 1.34)),
    ("hand.l", "lowerarm.l", (0.66, 0.0, 1.34), (0.76, 0.0, 1.34)),
    ("upperarm.r", "chest", (-0.17, 0.0, 1.34), (-0.42, 0.0, 1.34)),
    ("lowerarm.r", "upperarm.r", (-0.42, 0.0, 1.34), (-0.66, 0.0, 1.34)),
    ("hand.r", "lowerarm.r", (-0.66, 0.0, 1.34), (-0.76, 0.0, 1.34)),
    ("upperleg.l", "hips", (0.09, 0.0, 0.95), (0.09, 0.0, 0.52)),
    ("lowerleg.l", "upperleg.l", (0.09, 0.0, 0.52), (0.09, 0.0, 0.1)),
    ("foot.l", "lowerleg.l", (0.09, 0.0, 0.1), (0.09, -0.12, 0.02)),
    ("upperleg.r", "hips", (-0.09, 0.0, 0.95), (-0.09, 0.0, 0.52)),
    ("lowerleg.r", "upperleg.r", (-0.09, 0.0, 0.52), (-0.09, 0.0, 0.1)),
    ("foot.r", "lowerleg.r", (-0.09, 0.0, 0.1), (-0.09, -0.12, 0.02)),
)

# Base color, then the extras each material carries (`materials.set_extras`).
MATERIALS = {
    "Skin": ((0.93, 0.76, 0.68, 1.0), {"shade_color": (0.85, 0.55, 0.5), "character_shading": (0.05, 0.03, 0.4, 5.0)}),
    "Cloth": (
        (0.22, 0.27, 0.52, 1.0),
        {"shade_color": (0.4, 0.42, 0.6), "rim_regions": ((1.0, 1.0, 1.0), (1.5, 0.8, 0.5))},
    ),
    "Face": (
        (0.95, 0.8, 0.72, 1.0),
        {
            "shade_color": (0.9, 0.62, 0.58),
            "outline_color": (0.35, 0.18, 0.16),
            "character_shading": (0.1, 0.02, 0.3, 3.5),
        },
    ),
    "Hair": (
        (0.3, 0.2, 0.42, 1.0),
        {
            "outline_color": (0.12, 0.08, 0.2),
            "rim": {"width": 3.0, "strength": 0.5, "threshold": 0.12, "fresnel": 0.6, "light_gate": 0.7, "back_light": 0.2},
        },
    ),
}

SIDES = 12
HEAD_CENTER = (0.0, 0.0, 1.56)
HEAD_RADIUS = 0.11
HAIR_BAND = 1.58
# Data colors, as bytes over 255.
SKIN_DATA = (255, 255, 0)
HEM_DATA = (153, 204, 0)
FACE_DATA = (255, 128, 64)
HAIR_DATA = (230, 255, 102)
SMILE = 0.005


def parse_arguments():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(prog="test_character.py", description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, help="folder for test_character.glb (its sidecar stays as it is)")
    parser.add_argument("--installed", action="store_true", help="use the add-on installed in Blender, never the checkout's")
    parser.add_argument("--check", action="store_true", help="run Check on --out after the export")
    parser.add_argument("--pipeline", default="", help="the sapphire_pipeline binary (default: $SAPPHIRE_PIPELINE)")
    parser.add_argument("--bake", default="", help="bake the face's masks into this folder and draw their sheet")
    parser.add_argument("--hull", action="store_true", help="show the body's hull preview and confirm it")
    return parser.parse_args(argv)


def enable_addon(installed):
    """The add-on's package: the installed extension, enabled, or the checkout's, registered."""
    import addon_utils

    for module in addon_utils.modules():
        if module.__name__.split(".")[-1] == "sapphire":
            addon_utils.enable(module.__name__, default_set=True, handle_error=None)
            print(f"enabled the installed add-on {module.__name__} from {os.path.dirname(module.__file__)}")
            return sys.modules[module.__name__]
    if installed:
        raise SystemExit("no Sapphire add-on is installed in this Blender")
    sys.path.insert(0, SCRIPTS)
    import sapphire

    sapphire.register()
    print(f"registered the checkout's add-on from {os.path.dirname(sapphire.__file__)}")
    return sapphire


def empty_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0
    return scene


def armature(collection):
    data = bpy.data.armatures.new("Armature")
    obj = bpy.data.objects.new("Armature", data)
    collection.objects.link(obj)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.object.mode_set(mode="EDIT")
    for name, parent, head, tail in BONES:
        bone = data.edit_bones.new(name)
        bone.head, bone.tail = head, tail
        if parent is not None:
            bone.parent = data.edit_bones[parent]
            bone.use_connect = tuple(data.edit_bones[parent].tail) == head
    bpy.ops.object.mode_set(mode="OBJECT")
    return obj


def material(sapphire, name):
    found = bpy.data.materials.get(name)
    if found is not None:
        return found
    base, extras = MATERIALS[name]
    made = bpy.data.materials.new(name)
    made.diffuse_color = base
    made.node_tree.nodes.get("Principled BSDF").inputs["Base Color"].default_value = base
    made.use_backface_culling = True
    sapphire.materials.set_extras(made, **extras)
    return made


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


BONE_INDEX = {name: index for index, (name, *_rest) in enumerate(BONES)}
TORSO = (("hips", 0.0, 1.05), ("spine", 1.05, 1.2), ("chest", 1.2, 1.38), ("neck", 1.38, 1.46), ("head", 1.46, 2.0))


def chain_weights(s, chain, blend=0.04):
    """Weights along a chain of (bone, start, end) by the coordinate `s`, blended linearly across each
    joint over ±`blend`; the chain's ends hold past them."""
    weights = {}
    for index, (bone, start, end) in enumerate(chain):
        low = start - blend if index > 0 else -math.inf
        high = end + blend if index < len(chain) - 1 else math.inf
        if not low < s < high:
            continue
        weight = 1.0
        if index > 0:
            weight = min(weight, (s - low) / (2 * blend))
        if index < len(chain) - 1:
            weight = min(weight, (high - s) / (2 * blend))
        weights[bone] = weight
    total = sum(weights.values())
    return {bone: weight / total for bone, weight in weights.items()}


def tube(bm, rings, axis, u, slot, weigh):
    """An open tube of `SIDES` around `axis` through `rings` ((center, radius along u, radius along
    axis × u)), faces outward, UVs wrapped around it, each vertex weighted by `weigh(position)`.
    Returns its rings of vertices and its bands of faces, both in `rings`' order."""
    uv = bm.loops.layers.uv.verify()
    deform = bm.verts.layers.deform.verify()
    v = cross(axis, u)
    made = []
    for center, radius_u, radius_v in rings:
        ring = []
        for side in range(SIDES):
            angle = 2.0 * math.pi * side / SIDES
            c, s = math.cos(angle) * radius_u, math.sin(angle) * radius_v
            vertex = bm.verts.new(tuple(center[k] + c * u[k] + s * v[k] for k in range(3)))
            for bone, weight in weigh(vertex.co).items():
                vertex[deform][BONE_INDEX[bone]] = weight
            ring.append(vertex)
        made.append(ring)
    bands = []
    for k in range(len(rings) - 1):
        a, b, band = made[k], made[k + 1], []
        for side in range(SIDES):
            after = (side + 1) % SIDES
            face = bm.faces.new((a[side], a[after], b[after], b[side]))
            face.material_index = slot
            us = (side / SIDES, (side + 1) / SIDES)
            vs = (k / (len(rings) - 1), (k + 1) / (len(rings) - 1))
            for loop, coordinates in zip(face.loops, ((us[0], vs[0]), (us[1], vs[0]), (us[1], vs[1]), (us[0], vs[1]))):
                loop[uv].uv = coordinates
            band.append(face)
        bands.append(band)
    return made, bands


def rigid(bm, bone):
    deform = bm.verts.layers.deform.verify()
    for vertex in bm.verts:
        vertex[deform][BONE_INDEX[bone]] = 1.0


def triangulate(bm, marked=frozenset()):
    """Splits every quad corner 0 to 2, so the export never chooses a diagonal (Blender's exporter picks
    the shorter, and a sphere's quads tie), then orders the faces by their vertices: new faces fill the
    holes deleted ones left in an order that varies between runs. Returns the triangles made from `marked`
    faces, after the sort (which moves faces in memory, so a face reference from before it can name
    another face)."""
    made = bmesh.ops.triangulate(bm, faces=bm.faces[:], quad_method="FIXED", ngon_method="EAR_CLIP")
    kept = {face for face in marked if face.is_valid}
    kept |= {face for face, source in made["face_map"].items() if source in marked}
    bm.verts.index_update()
    count = len(bm.verts)

    def by_vertices(face):
        # A triangle's sorted vertex indices as one number, as `sort` wants.
        a, b, c = sorted(vertex.index for vertex in face.verts)
        return float((a * count + b) * count + c)

    keys = {by_vertices(face) for face in kept}
    bm.faces.sort(key=by_vertices)
    bm.faces.index_update()
    return [face for face in bm.faces if by_vertices(face) in keys]


def mesh_object(name, bm, materials, collection, rig):
    """A mesh object from `bm`, smooth-shaded, a vertex group a bone (in `BONES`' order, which the
    deform layer's indices follow), parented to `rig` with an armature modifier."""
    mesh = bpy.data.meshes.new(name)
    bm.to_mesh(mesh)
    bm.free()
    for slot in materials:
        mesh.materials.append(slot)
    mesh.shade_smooth()
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    for bone, *_rest in BONES:
        obj.vertex_groups.new(name=bone)
    obj.parent = rig
    obj.modifiers.new("Armature", "ARMATURE").object = rig
    return obj


def set_data(sapphire, mesh, data, vertices=None):
    for channel, value in zip(("AO", "WIDTH", "Z_OFFSET"), data):
        sapphire.mesh_data.set_data_channel(mesh, channel, value / 255.0, vertices)


def body(sapphire, collection, rig):
    bm = bmesh.new()
    up, down, x = (0.0, 0.0, 1.0), (0.0, 0.0, -1.0), (1.0, 0.0, 0.0)

    def along_torso(co):
        return chain_weights(co.z, TORSO)

    torso = [((0.0, 0.0, h), rx, ry) for h, rx, ry in ((0.92, 0.13, 0.09), (1.0, 0.13, 0.09), (1.1, 0.12, 0.085), (1.2, 0.14, 0.095), (1.3, 0.15, 0.1), (1.38, 0.12, 0.08))]
    tube(bm, torso, up, x, 1, along_torso)
    skirt = [((0.0, 0.0, h), rx, ry) for h, rx, ry in ((0.98, 0.16, 0.11), (0.85, 0.22, 0.17), (0.72, 0.27, 0.22), (0.66, 0.29, 0.24))]
    skirt_rings, skirt_bands = tube(bm, skirt, down, x, 1, along_torso)
    neck = [((0.0, 0.0, h), 0.045, 0.045) for h in (1.36, 1.42, 1.48)]
    tube(bm, neck, up, x, 0, along_torso)
    for sign, side in ((1.0, "l"), (-1.0, "r")):
        arm_chain = (("chest", 0.0, 0.17), (f"upperarm.{side}", 0.17, 0.42), (f"lowerarm.{side}", 0.42, 0.66), (f"hand.{side}", 0.66, 1.0))
        arm = [((sign * s, 0.0, 1.34), r, r) for s, r in ((0.15, 0.05), (0.3, 0.045), (0.42, 0.04), (0.54, 0.038), (0.66, 0.032), (0.76, 0.03))]
        tube(bm, arm, (sign, 0.0, 0.0), (0.0, 1.0, 0.0), 0, lambda co, chain=arm_chain: chain_weights(abs(co.x), chain))
        leg_chain = (("hips", -2.0, -0.95), (f"upperleg.{side}", -0.95, -0.52), (f"lowerleg.{side}", -0.52, -0.1), (f"foot.{side}", -0.1, 0.0))
        leg = [((sign * 0.09, 0.0, h), r, r) for h, r in ((0.95, 0.075), (0.75, 0.065), (0.52, 0.055), (0.3, 0.05), (0.1, 0.04), (0.04, 0.04))]
        tube(bm, leg, down, x, 0, lambda co, chain=leg_chain: chain_weights(-co.z, chain))
    hem_band = triangulate(bm, set(skirt_bands[-1]))
    bm.verts.index_update()
    bm.faces.index_update()
    hem = [vertex.index for vertex in skirt_rings[-1]]
    band = [face.index for face in hem_band]
    obj = mesh_object("Body", bm, [material(sapphire, "Skin"), material(sapphire, "Cloth")], collection, rig)
    mesh = obj.data
    sapphire.mesh_data.assign_material_id(mesh, 0)
    sapphire.mesh_data.assign_material_id(mesh, 1, band)
    set_data(sapphire, mesh, SKIN_DATA)
    set_data(sapphire, mesh, HEM_DATA, hem)
    sapphire.mesh_data.set_outline_normals(obj)
    return obj


def sphere_half(bm, front):
    """The front (y ≤ 0) or back (y ≥ 0) half of the head's sphere."""
    made = bmesh.ops.create_uvsphere(bm, u_segments=16, v_segments=10, radius=HEAD_RADIUS, calc_uvs=True)
    bmesh.ops.translate(bm, vec=HEAD_CENTER, verts=made["verts"])
    drop = [vertex for vertex in made["verts"] if (vertex.co.y > 1e-6 if front else vertex.co.y < -1e-6)]
    bmesh.ops.delete(bm, geom=drop, context="VERTS")
    triangulate(bm)


def head_parts(sapphire, collection, rig, proxy):
    objects = []
    for name, front, data in (("Head", False, SKIN_DATA), ("Face", True, FACE_DATA)):
        bm = bmesh.new()
        sphere_half(bm, front)
        if front:
            uv = bm.loops.layers.uv.verify()
            # From the front, mirrored about u = 0.5, as the face map wants.
            for face in bm.faces:
                for loop in face.loops:
                    co = loop.vert.co
                    loop[uv].uv = (0.5 - 0.5 * co.x / HEAD_RADIUS, 0.5 + 0.5 * (co.z - HEAD_CENTER[2]) / HEAD_RADIUS)
        rigid(bm, "head")
        slot = material(sapphire, "Face" if front else "Skin")
        obj = mesh_object(name, bm, [slot], collection, rig)
        mesh = obj.data
        sapphire.mesh_data.assign_material_id(mesh, 0)
        set_data(sapphire, mesh, data)
        sapphire.mesh_data.set_outline_normals(obj, proxy)
        objects.append(obj)
    face = objects[1]
    face.shape_key_add(name="Basis", from_mix=False)
    smile = face.shape_key_add(name="smile", from_mix=False)
    for index, vertex in enumerate(face.data.vertices):
        co = vertex.co
        if co.z < HEAD_CENTER[2] - 0.03 and abs(co.x) < 0.06 and co.y < -0.07:
            smile.data[index].co = (co.x, co.y, co.z + SMILE)
    smile.value = 0.0
    return objects


def hair(sapphire, collection, rig, proxy):
    bm = bmesh.new()
    made = bmesh.ops.create_icosphere(bm, subdivisions=2, radius=HEAD_RADIUS + 0.02)
    bmesh.ops.translate(bm, vec=(HEAD_CENTER[0], HEAD_CENTER[1] + 0.01, HEAD_CENTER[2]), verts=made["verts"])
    # A cap: the sphere above the brow, open at its edge, leaving the face clear.
    drop = [vertex for vertex in made["verts"] if vertex.co.z < 1.55 or (vertex.co.y < -0.04 and vertex.co.z < 1.62)]
    bmesh.ops.delete(bm, geom=drop, context="VERTS")
    triangulate(bm)
    uv = bm.loops.layers.uv.verify()
    for face in bm.faces:
        for loop in face.loops:
            co = loop.vert.co
            loop[uv].uv = (0.5 + 0.5 * math.atan2(co.y, co.x) / math.pi, (co.z - 1.5) / 0.25)
    bm.faces.index_update()
    band = [face.index for face in bm.faces if face.calc_center_median().z < HAIR_BAND]
    rigid(bm, "head")
    obj = mesh_object("Hair", bm, [material(sapphire, "Hair")], collection, rig)
    obj.data.shade_flat()
    mesh = obj.data
    sapphire.mesh_data.assign_material_id(mesh, 0)
    sapphire.mesh_data.assign_material_id(mesh, 2, band)
    set_data(sapphire, mesh, HAIR_DATA)
    sapphire.mesh_data.set_outline_normals(obj, proxy)
    return obj


def proxy(name, collection, center, radii):
    """A smooth ellipsoid to transfer outline normals from; removed once they are."""
    mesh = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_uvsphere(bm, u_segments=48, v_segments=24, radius=1.0)
    bm.to_mesh(mesh)
    bm.free()
    mesh.shade_smooth()
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    obj.location = center
    obj.scale = radii
    bpy.context.view_layer.update()
    return obj


def build(sapphire):
    collection = bpy.context.scene.collection
    rig = armature(collection)
    head_proxy = proxy("HeadProxy", collection, HEAD_CENTER, (HEAD_RADIUS,) * 3)
    # Inside the cap, so each of its vertices has one nearest point on it.
    hair_proxy = proxy("HairProxy", collection, (0.0, 0.01, 1.58), (0.1, 0.11, 0.09))
    body(sapphire, collection, rig)
    head_parts(sapphire, collection, rig, head_proxy)
    hair(sapphire, collection, rig, hair_proxy)
    for removed in (head_proxy, hair_proxy):
        mesh = removed.data
        bpy.data.objects.remove(removed)
        bpy.data.meshes.remove(mesh)


def confirm_hull(sapphire):
    """Shows the body's hull preview, and confirms the evaluated body is the body twice."""
    obj = bpy.data.objects["Body"]
    sapphire.mesh_data.show_hull(obj, bpy.context.scene.sapphire.hull_pixel)
    evaluated = obj.evaluated_get(bpy.context.evaluated_depsgraph_get())
    count, body_count = len(evaluated.data.vertices), len(obj.data.vertices)
    if count != 2 * body_count:
        raise SystemExit(f"the hull preview drew {count} vertices for a body of {body_count}")
    # The body comes first, then the hull. Its widest push is the cloth's width (5.5 px, the default) at
    # a data color G of 1, along a unit outline normal.
    pixel = bpy.context.scene.sapphire.hull_pixel
    pushed = max((evaluated.data.vertices[body_count + i].co - obj.data.vertices[i].co).length for i in range(body_count))
    expected = sapphire.mesh_data.DEFAULT_OUTLINE_WIDTH * pixel
    if abs(pushed - expected) > 1e-5:
        raise SystemExit(f"the hull preview pushed {pushed} m at most, not {expected}")
    print(f"hull preview: {count} vertices, the body's {body_count} twice, pushed {pushed * 1000:.2f} mm at most")


def main():
    args = parse_arguments()
    if args.pipeline:
        os.environ["SAPPHIRE_PIPELINE"] = os.path.abspath(args.pipeline)
    empty_scene()
    sapphire = enable_addon(args.installed)
    build(sapphire)
    if args.hull:
        confirm_hull(sapphire)
    out = os.path.abspath(args.out)
    result = bpy.ops.sapphire.export(filepath=os.path.join(out, "test_character.glb"))
    if result != {"FINISHED"}:
        raise SystemExit(f"the export didn't finish: {result}")
    print(f"exported {os.path.join(out, 'test_character.glb')}")
    if args.bake:
        bpy.context.view_layer.objects.active = bpy.data.objects["Face"]
        bake = os.path.abspath(args.bake)
        result = bpy.ops.sapphire.bake_face_masks(out=bake, size=128, horizontal="0:180:30", overwrite=True)
        if result != {"FINISHED"}:
            raise SystemExit(f"the face bake didn't finish: {result}")
        document = os.path.join(bake, "face.facesdf.ron")
        result = bpy.ops.sapphire.face_sheet(document=document, angles="30,60,90")
        if result != {"FINISHED"}:
            raise SystemExit(f"the face sheet didn't finish: {result}")
    if args.check:
        settings = bpy.context.scene.sapphire
        settings.content_root = out
        result = bpy.ops.sapphire.check()
        if result != {"FINISHED"}:
            raise SystemExit(f"Check didn't finish: {result}")
        failures = [finding for finding in settings.findings if finding.severity == "error" and not finding.waived]
        if failures or not any(row.role == "hero" for row in settings.measured):
            raise SystemExit(f"the check didn't pass the character at the hero role: {settings.check_summary}")


if __name__ == "__main__":
    main()
