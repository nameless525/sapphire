"""Bakes the masks Sapphire's anime character maps start from (M1 A7).

Run it headless on a character's .blend; it never saves the .blend:

    blender character.blend --background --python tools/blender/bake_character_masks.py -- \
        --face Face --out content/characters/heroine/face --size 1024 --vertical 0:90:15

For --face, it writes threshold masks for the SDF face map: masks/h_*.png for a light
turning from head-forward toward head-right, masks/v_*.png (with --vertical) for one rising
toward overhead, and face.facesdf.ron listing them. For each --body object, it bakes ambient
occlusion and material IDs and writes <name>_ao.png, <name>_ids.png and <name>.ilm.ron.
docs/blender-mask-bake.md describes the whole recipe.

Masks come from the shading normals (custom normals and modifiers included) baked in object
space and turned into world space; head-forward and head-up are world directions, by default
-Y and +Z, which is how Blender characters (VRM imports included) face the front view.
Armatures are put in their rest pose while baking.
"""

import argparse
import math
import os
import sys

import bpy
import numpy as np

# Levels of material IDs in the ILM map's alpha (sapphire_assets::character_maps).
MATERIAL_IDS = 8


def parse_arguments():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(
        prog="bake_character_masks.py",
        description="Bake threshold masks and ILM input masks for Sapphire's anime character maps.",
    )
    parser.add_argument("--out", required=True, help="directory for the masks and documents")
    parser.add_argument("--face", help="face mesh object: bake SDF threshold masks")
    parser.add_argument(
        "--body",
        action="append",
        default=[],
        help="mesh object to bake ILM inputs for (AO, material IDs); repeat for more",
    )
    parser.add_argument("--size", type=int, default=1024, help="mask width and height in texels")
    parser.add_argument("--uv-map", default="", help="UV map to bake into (default: the active one)")
    parser.add_argument(
        "--horizontal",
        default="0:180:10",
        help="angles of the horizontal masks: start:stop:step or a comma list, in degrees",
    )
    parser.add_argument("--vertical", default="", help="angles of the vertical masks, as above (default: none)")
    parser.add_argument("--forward", default="0,-1,0", help="head-forward as a world direction")
    parser.add_argument("--up", default="0,0,1", help="head-up as a world direction")
    parser.add_argument(
        "--terminator",
        type=float,
        default=90.0,
        help="a texel is lit while the light is less than this many degrees past its normal",
    )
    parser.add_argument("--margin", type=int, default=8, help="texels baked past UV island edges")
    parser.add_argument("--ao-samples", type=int, default=64, help="Cycles samples for the AO bake")
    parser.add_argument("--ao-distance", type=float, default=0.1, help="AO ray length in meters")
    parser.add_argument(
        "--material-ids",
        default="",
        help="material=ID pairs, comma-separated (default: material slot order)",
    )
    parser.add_argument("--overwrite", action="store_true", help="replace existing masks and documents")
    arguments = parser.parse_args(argv)
    if not arguments.face and not arguments.body:
        parser.error("nothing to bake: give --face, --body or both")
    return arguments


def parse_angles(text):
    """Angles from 'start:stop:step' (stop included) or 'a,b,c'."""
    if not text:
        return []
    if ":" in text:
        start, stop, step = (float(value) for value in text.split(":"))
        count = int(math.floor((stop - start) / step + 1e-6)) + 1
        angles = [start + index * step for index in range(count)]
    else:
        angles = [float(value) for value in text.split(",")]
    if len(angles) < 2 or any(b <= a for a, b in zip(angles, angles[1:])) or not 0 <= angles[0] <= angles[-1] <= 180:
        raise SystemExit(f"angles {text!r}: need at least two, increasing, within 0 to 180 degrees")
    return angles


def parse_vector(text):
    vector = np.array([float(value) for value in text.split(",")], dtype=np.float64)
    if vector.shape != (3,) or not np.linalg.norm(vector) > 0:
        raise SystemExit(f"{text!r} is not a direction like 0,-1,0")
    return vector / np.linalg.norm(vector)


class Session:
    """Changes to the open file, undone on exit in reverse order."""

    def __init__(self):
        self.undo = []

    def set(self, owner, name, value):
        previous = getattr(owner, name)
        self.undo.append(lambda: setattr(owner, name, previous))
        setattr(owner, name, value)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        for undo in reversed(self.undo):
            undo()


def mesh_object(name):
    obj = bpy.data.objects.get(name)
    if obj is None or obj.type != "MESH":
        raise SystemExit(f"no mesh object named {name!r}")
    return obj


def prepare(session, objects):
    """Cycles on the CPU, armatures at rest, nothing selected."""
    scene = bpy.context.scene
    session.set(scene.render, "engine", "CYCLES")
    session.set(scene.cycles, "device", "CPU")
    for obj in objects:
        for modifier in obj.modifiers:
            if modifier.type == "ARMATURE" and modifier.object is not None:
                session.set(modifier.object.data, "pose_position", "REST")
    view_layer = bpy.context.view_layer
    for obj in view_layer.objects:
        if obj.select_get():
            obj.select_set(False)
            session.undo.append(lambda obj=obj: obj.select_set(True))
    session.set(view_layer.objects, "active", None)


def bake(session, obj, size, uv_map, margin, pass_type, slot_colors=None, samples=1, **options):
    """Bakes `pass_type` of `obj` into a new float image; returns its RGBA texels, bottom row
    first, as Blender stores images. `slot_colors` gives each material slot an emission color
    for EMIT bakes.

    The bake runs on a copy of the object with temporary materials, so the original's
    materials are never touched; the original is hidden from rendering meanwhile, so it
    doesn't occlude its copy."""
    scene = bpy.context.scene
    image = bpy.data.images.new("sapphire_bake", size, size, alpha=True, float_buffer=True, is_data=True)
    copy = obj.copy()
    copy.data = obj.data.copy()
    scene.collection.objects.link(copy)
    copy.hide_render = False
    copy.hide_viewport = False
    copy.hide_set(False)
    materials = []
    try:
        for index in range(max(1, len(copy.material_slots))):
            material = bpy.data.materials.new(f"sapphire_bake_{index}")
            if material.node_tree is None:
                # Blender before 5.0 makes materials without a node tree.
                material.use_nodes = True
            nodes, links = material.node_tree.nodes, material.node_tree.links
            nodes.clear()
            output = nodes.new("ShaderNodeOutputMaterial")
            if slot_colors is not None:
                shader = nodes.new("ShaderNodeEmission")
                shader.inputs["Color"].default_value = (*slot_colors[index], 1.0)
                shader.inputs["Strength"].default_value = 1.0
            else:
                shader = nodes.new("ShaderNodeBsdfDiffuse")
            links.new(shader.outputs[0], output.inputs["Surface"])
            target = nodes.new("ShaderNodeTexImage")
            target.image = image
            nodes.active = target
            materials.append(material)
        if copy.material_slots:
            for slot, material in zip(copy.material_slots, materials):
                slot.material = material
        else:
            copy.data.materials.append(materials[0])

        with Session() as local:
            local.set(obj, "hide_render", True)
            local.set(scene.cycles, "samples", samples)
            copy.select_set(True)
            bpy.context.view_layer.objects.active = copy
            bpy.ops.object.bake(
                type=pass_type,
                margin=margin,
                margin_type="EXTEND",
                use_clear=True,
                uv_layer=uv_map,
                target="IMAGE_TEXTURES",
                **options,
            )
            bpy.context.view_layer.objects.active = None
        texels = np.empty(size * size * 4, dtype=np.float32)
        image.pixels.foreach_get(texels)
        return texels.reshape(size * size, 4)
    finally:
        mesh = copy.data
        bpy.data.objects.remove(copy)
        bpy.data.meshes.remove(mesh)
        for material in materials:
            bpy.data.materials.remove(material)
        bpy.data.images.remove(image)


def coverage(session, obj, args):
    """Texels inside the UV islands (and their margin)."""
    white = [(1.0, 1.0, 1.0)] * max(1, len(obj.material_slots))
    texels = bake(session, obj, args.size, args.uv_map, args.margin, "EMIT", slot_colors=white)
    return texels[:, 0] > 0.5


def world_normals(session, obj, args):
    """Unit shading normals per texel, in world space."""
    texels = bake(
        session,
        obj,
        args.size,
        args.uv_map,
        args.margin,
        "NORMAL",
        normal_space="OBJECT",
        normal_r="POS_X",
        normal_g="POS_Y",
        normal_b="POS_Z",
    )
    local = texels[:, :3].astype(np.float64) * 2.0 - 1.0
    # The bake stores the world normal taken into object space as a direction (by the
    # inverse of the object's linear part), so the object's linear part takes it back; under
    # non-uniform scale that differs from the inverse transpose normals usually need.
    to_world = np.array(obj.matrix_world.to_3x3(), dtype=np.float64)
    world = local @ to_world.T
    length = np.linalg.norm(world, axis=1, keepdims=True)
    return world / np.maximum(length, 1e-12)


def check_outputs(paths, overwrite):
    """Stops before anything is baked if an output exists and may not be replaced."""
    existing = [path for path in paths if os.path.exists(path)]
    if existing and not overwrite:
        listed = "\n  ".join(existing)
        raise SystemExit(f"these exist; pass --overwrite to replace them (painted changes are lost):\n  {listed}")


def remove_stale_masks(out, keep):
    """Removes masks an earlier bake wrote that this one doesn't: left behind, a cook would take
    them for textures of their own."""
    directory = os.path.join(out, "masks")
    if not os.path.isdir(directory):
        return
    for name in sorted(os.listdir(directory)):
        path = os.path.join(directory, name)
        if name[:2] in ("h_", "v_") and name.endswith(".png") and path not in keep:
            os.remove(path)
            print(f"removed stale {path}")


def save_png(path, gray, size):
    """Writes `gray` (one value in [0, 1] per texel, bottom row first) as an 8-bit PNG, gray in
    RGB."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    image = bpy.data.images.new("sapphire_mask", size, size, alpha=False, float_buffer=False, is_data=True)
    try:
        rgba = np.ones((size * size, 4), dtype=np.float32)
        rgba[:, :3] = np.clip(gray, 0.0, 1.0)[:, None]
        image.pixels.foreach_set(rgba.ravel())
        image.filepath_raw = path
        image.file_format = "PNG"
        image.save()
    finally:
        bpy.data.images.remove(image)
    print(f"wrote {path}")


def save_document(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as file:
        file.write(text)
    print(f"wrote {path}")


def mask_name(prefix, angle):
    return f"masks/{prefix}_{angle:06.2f}.png"


def face_channels(args):
    """(channel, file prefix, angles) of each channel the face map has."""
    channels = [("horizontal", "h", parse_angles(args.horizontal))]
    if args.vertical:
        channels.append(("vertical", "v", parse_angles(args.vertical)))
    return channels


def face_outputs(args):
    masks = [mask_name(prefix, angle) for _, prefix, angles in face_channels(args) for angle in angles]
    return [os.path.join(args.out, path) for path in masks + ["face.facesdf.ron"]]


def body_stem(name):
    return name.lower().replace(" ", "_")


def body_outputs(args, name):
    stem = body_stem(name)
    return [os.path.join(args.out, f"{stem}{suffix}") for suffix in ("_ao.png", "_ids.png", ".ilm.ron")]


def bake_face(session, args):
    obj = mesh_object(args.face)
    forward = parse_vector(args.forward)
    up = parse_vector(args.up)
    up = up - forward * np.dot(up, forward)
    if np.linalg.norm(up) < 1e-6:
        raise SystemExit("--forward and --up must not be parallel")
    up /= np.linalg.norm(up)
    right = np.cross(forward, up)
    covered = coverage(session, obj, args)
    normals = world_normals(session, obj, args)

    sides = {"horizontal": right, "vertical": up}
    lines = [
        "// Written by tools/blender/bake_character_masks.py from the shading normals of",
        f"// {obj.name!r}. Paint over the masks, keeping each inside the one before it, then cook.",
        "(",
        '    type: "sapphire::pipeline::FaceSdfMap",',
        "    version: 1,",
        "    data: (",
    ]
    for channel, prefix, angles in face_channels(args):
        # The normal's angle from head-forward toward the side the light turns to, in the
        # plane it turns in.
        azimuth = np.degrees(np.arctan2(normals @ sides[channel], normals @ forward))
        lines.append(f"        {channel}: [")
        for angle in angles:
            # Lit while the light is less than the terminator past the normal. That only ever
            # shrinks the lit region as the angle grows, so the masks are monotonic.
            lit = covered & (angle < azimuth + args.terminator)
            path = mask_name(prefix, angle)
            save_png(os.path.join(args.out, path), lit.astype(np.float32), args.size)
            lines.append(f'            (angle: {float(angle)!r}, path: "{path}"),')
        lines.append("        ],")
    lines += ["    ),", ")", ""]
    document = "\n".join(lines)
    save_document(os.path.join(args.out, "face.facesdf.ron"), document)


def material_ids(obj, text):
    """Each material slot's ID: from `text` by material name, else the slot's index."""
    named = {}
    for pair in filter(None, (part.strip() for part in text.split(","))):
        name, _, value = pair.rpartition("=")
        named[name] = int(value)
    ids = []
    for index, slot in enumerate(obj.material_slots or [None]):
        name = slot.material.name if slot is not None and slot.material is not None else ""
        material_id = named.get(name, index)
        if not 0 <= material_id < MATERIAL_IDS:
            raise SystemExit(f"material {name!r}: ID {material_id} isn't below {MATERIAL_IDS}")
        ids.append(material_id)
    return ids


def bake_body(session, args, name):
    obj = mesh_object(name)
    ids = material_ids(obj, args.material_ids)
    scene = bpy.context.scene
    with Session() as local:
        if scene.world is not None:
            local.set(scene.world.light_settings, "distance", args.ao_distance)
        ao = bake(session, obj, args.size, args.uv_map, args.margin, "AO", samples=args.ao_samples)[:, 0]
    # IDs as the gray levels the ILM map stores, k / 7, emitted flat per material slot.
    levels = [(k / (MATERIAL_IDS - 1),) * 3 for k in ids]
    gray = bake(session, obj, args.size, args.uv_map, args.margin, "EMIT", slot_colors=levels)[:, 0]
    gray = np.round(gray * (MATERIAL_IDS - 1)) / (MATERIAL_IDS - 1)
    stem = body_stem(name)
    save_png(os.path.join(args.out, f"{stem}_ao.png"), ao, args.size)
    save_png(os.path.join(args.out, f"{stem}_ids.png"), gray, args.size)
    document = (
        "// Written by tools/blender/bake_character_masks.py. AO darkens the shadow bias (0.5 is\n"
        "// neutral); paint a specular mask and add `specular_mask: Image(path: ...)`.\n"
        "(\n"
        '    type: "sapphire::pipeline::IlmMap",\n'
        "    version: 1,\n"
        "    data: (\n"
        f'        shadow_bias: Image(path: "{stem}_ao.png", range: (0.0, 0.5)),\n'
        f'        material_id: Image(path: "{stem}_ids.png"),\n'
        "    ),\n)\n"
    )
    save_document(os.path.join(args.out, f"{stem}.ilm.ron"), document)


def main():
    args = parse_arguments()
    objects = [mesh_object(name) for name in ([args.face] if args.face else []) + args.body]
    outputs = face_outputs(args) if args.face else []
    for name in args.body:
        outputs += body_outputs(args, name)
    check_outputs(outputs, args.overwrite)
    if args.face:
        remove_stale_masks(args.out, set(outputs))
    with Session() as session:
        prepare(session, objects)
        if args.face:
            bake_face(session, args)
        for name in args.body:
            bake_body(session, args, name)


if __name__ == "__main__":
    main()
