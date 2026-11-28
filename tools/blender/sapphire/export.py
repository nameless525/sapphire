# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""The export: Blender's own glTF exporter, with the settings Sapphire's importer reads
(`ENGINE_SETTINGS`), and around it what Blender 5.2's exporter gets wrong for this data:

- It writes custom vector attributes as they are, where it moves a skinned mesh's normals into its
  armature's world and turns every normal from Blender's Z-up into glTF's Y-up, so `_OUTLINE_NORMAL` is
  given the same transform for the export's length and put back after (`outline_normals_in_gltf_space`).
  A mesh two objects would turn differently is refused.
- It writes a mesh's data color into its first material's primitive only, and every other material's
  corners white (`every_materials_data_color`): a patch of a private method of its own, with a warning
  on a Blender that has none.
- The hull preview's modifiers are switched off while it runs, so applying modifiers never exports them.

Every export marks its glTF scene's extras with the add-on's version (`sapphire_addon`), so the importer can
tell this add-on's files from Blender's own and refuse a version known to export wrong (`gltf.rs`).

Send (M6 C4) is the export into the content tree: the file must be in the scene's content folder, where a
running `cargo xtask content --watch` cooks it, and the running app's hot reload or live link shows it. Every
export writes a hidden file and renames it into place, so a watch never cooks half a file.

Each run writes the same bytes for the same scene and Blender version: the exporter's output depends on
the scene alone, and the scene's add-on settings stay out of its extras (`settings`)."""

import contextlib
import os
import tomllib

import bpy
import numpy as np
from bpy.props import StringProperty

from . import mesh_data

# The add-on's version, from its manifest, and the glTF scene extras key that carries it.
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "blender_manifest.toml"), "rb") as _manifest:
    VERSION = tomllib.load(_manifest)["version"]
MARK = "sapphire_addon"

# Blender's glTF exporter's options, as Sapphire's importer reads glTF (gltf.rs): a binary file, Y-up,
# normals and UVs, the data color by name as `COLOR_0`, custom attributes (`_MATERIAL_ID`,
# `_OUTLINE_NORMAL`), material extras, skins and morph targets, no cameras, lights or tangents, and no
# fake vertex colors on meshes without the data color.
ENGINE_SETTINGS = {
    "export_format": "GLB",
    "export_yup": True,
    "export_texcoords": True,
    "export_normals": True,
    "export_tangents": False,
    "export_attributes": True,
    "export_vertex_color": "NAME",
    "export_vertex_color_name": mesh_data.DATA_COLOR,
    "export_all_vertex_colors": False,
    "export_active_vertex_color_when_no_material": False,
    "export_materials": "EXPORT",
    "export_extras": True,
    "export_cameras": False,
    "export_lights": False,
    "export_skins": True,
    "export_morph": True,
    "export_morph_normal": True,
    "export_def_bones": False,
    "export_leaf_bone": False,
    "export_draco_mesh_compression_enable": False,
    "will_save_settings": False,
    "check_existing": False,
}


def _z_up_to_y_up(vectors):
    """Blender's (x, y, z) as glTF's (x, z, -y), as the exporter turns positions and normals."""
    return np.stack([vectors[:, 0], vectors[:, 2], -vectors[:, 1]], axis=1)


def _instanced(collection, found):
    """Every object of `collection`, its child collections included, into `found`, and through any object of
    them that instances a collection, that collection's too: what the exporter writes for an instance."""
    for obj in collection.all_objects:
        if obj not in found:
            found.add(obj)
            if obj.instance_type == "COLLECTION" and obj.instance_collection is not None:
                _instanced(obj.instance_collection, found)


def exported_objects(selected_only):
    """The objects an export writes, in name order: the selection, or every scene's, and through each that
    instances a collection, that collection's objects, as the exporter walks them."""
    found = set()
    for obj in bpy.context.selected_objects if selected_only else (o for sc in bpy.data.scenes for o in sc.objects):
        found.add(obj)
        if obj.instance_type == "COLLECTION" and obj.instance_collection is not None:
            _instanced(obj.instance_collection, found)
    return sorted(found, key=lambda obj: (obj.name, obj.library.filepath if obj.library else ""))


def skinning_armature(obj, objects):
    """The armature Blender 5.2's glTF exporter skins `obj` to, with skins on: its last armature modifier's
    object, unless `obj` is parented to one of that armature's bones (a rigid part), or the armature isn't
    among the exported `objects` or has no bones, where the exporter writes `obj` unskinned."""
    modifiers = [modifier for modifier in obj.modifiers if modifier.type == "ARMATURE"]
    armature = modifiers[-1].object if modifiers else None
    if armature is None or armature not in objects or len(armature.data.bones) == 0:
        return None
    if obj.parent_type == "BONE" and obj.parent is not None and obj.parent.name == armature.name:
        return None
    return armature


def normal_transform(obj, objects):
    """The matrix the exporter turns `obj`'s normals by before Y-up. A skinned mesh's positions and normals
    are written in its armature's world: its normals by the armature's world 3×3 times the inverse transpose
    of the object's matrix relative to the armature. Any other mesh's node carries its transform, so its
    normals go as they are. Built from Blender's own matrices, as the exporter builds it. A skinned object
    mirrored by its transform is refused: the exporter bakes the mirror into its positions without turning
    its triangles round, so it would arrive inside out."""
    armature = skinning_armature(obj, objects)
    if armature is None:
        return np.identity(3)
    relative = armature.matrix_world.inverted_safe() @ obj.matrix_world
    if obj.matrix_world.to_3x3().determinant() < 0.0:
        raise mesh_data.MeshDataError(
            f"{obj.name} is skinned and mirrored by its transform (a negative scale), so it would export inside "
            "out: apply its scale (Object > Apply > Scale)"
        )
    return np.array(armature.matrix_world.to_3x3() @ relative.to_3x3().inverted_safe().transposed())


def outline_transforms(objects):
    """Each mesh with `_OUTLINE_NORMAL` among `objects`' with the one transform its objects give its normals,
    as (mesh, transform) pairs. A mesh shared by objects that would turn its normals differently can't hold
    both answers, and is refused naming them; transforms that differ only in scale agree, since the normals
    are normalized. A mesh linked from another file can't be written to, and is refused naming it. Meshes are
    told apart as datablocks, so a local and a linked mesh of one name stay two."""
    found = {}
    for obj in objects:
        if obj.type != "MESH":
            continue
        mesh = obj.data
        layer = mesh.attributes.get(mesh_data.OUTLINE_NORMAL)
        if layer is None or layer.data_type != "FLOAT_VECTOR":
            continue
        if mesh.library is not None:
            raise mesh_data.MeshDataError(
                f"{mesh.name} (linked from {mesh.library.filepath}, through {obj.name}) has "
                f"{mesh_data.OUTLINE_NORMAL}, which the export must turn into the file's space but can't write "
                "on linked data: make it local (Object > Relations > Make Local), or export it from its own file"
            )
        transform = normal_transform(obj, objects)
        key = mesh.as_pointer()
        if key not in found:
            found[key] = (mesh, obj, transform)
            continue
        _, first, kept = found[key]
        if not np.allclose(transform / np.linalg.norm(transform), kept / np.linalg.norm(kept), rtol=0.0, atol=1e-6):
            raise mesh_data.MeshDataError(
                f"{mesh.name}: the objects {first.name} and {obj.name} share this mesh at different transforms, "
                f"so its {mesh_data.OUTLINE_NORMAL} can't follow both their normals: give each its own mesh "
                "(Object > Relations > Make Single User > Object & Data)"
            )
    # Any other mesh that carries outline normals is turned Y-up as it stands, in case a path to it was missed.
    for mesh in bpy.data.meshes:
        layer = mesh.attributes.get(mesh_data.OUTLINE_NORMAL)
        if mesh.library is None and layer is not None and layer.data_type == "FLOAT_VECTOR":
            found.setdefault(mesh.as_pointer(), (mesh, None, np.identity(3)))
    return [(mesh, transform) for mesh, _obj, transform in found.values()]


@contextlib.contextmanager
def at_export_frame():
    """The scene at the frame the exporter gathers at, frame 0 (it asks for the current frame only with
    `export_current_frame`, which `ENGINE_SETTINGS` leaves off), for the block's length, so the transforms read
    here are the ones it writes with; the frame put back after."""
    scene = bpy.context.scene
    frame, subframe = scene.frame_current, scene.frame_subframe
    scene.frame_set(0)
    try:
        yield
    finally:
        scene.frame_set(frame, subframe=subframe)


@contextlib.contextmanager
def outline_normals_in_gltf_space(objects):
    """The `_OUTLINE_NORMAL` of each mesh of `objects` in the space the exporter writes its `NORMAL` in, while
    the block runs, put back after: turned by `normal_transform`, normalized, then Y-up. Read at the export's
    frame (`at_export_frame`)."""
    transforms = outline_transforms(objects)
    kept = []
    try:
        for mesh, transform in transforms:
            layer = mesh.attributes[mesh_data.OUTLINE_NORMAL]
            values = np.empty(len(layer.data) * 3, dtype=np.float32)
            layer.data.foreach_get("vector", values)
            kept.append((mesh, values))
            turned = values.reshape(-1, 3).astype(np.float64) @ transform.T
            lengths = np.linalg.norm(turned, axis=1, keepdims=True)
            turned = np.divide(turned, lengths, out=turned, where=lengths > 0.0)
            layer.data.foreach_set("vector", _z_up_to_y_up(turned).astype(np.float32).ravel())
            mesh.update()
        yield
    finally:
        for mesh, values in kept:
            mesh.attributes[mesh_data.OUTLINE_NORMAL].data.foreach_set("vector", values)
            mesh.update()


@contextlib.contextmanager
def every_materials_data_color(objects, warnings):
    """Works around Blender 5.2's glTF exporter writing a mesh's vertex colors into its first material's
    primitive only: for every later material it records the color attribute's name where the glTF
    attribute's (`COLOR_0`) belongs, then writes those corners white. Puts the glTF attribute back before
    the colors are written, for the block's length. The method patched is the exporter's own, private: on
    a Blender without it (or without its module or class) the export goes on unpatched, with a line in
    `warnings` naming each mesh of `objects` it may write wrong (one with a data color and more than one
    material)."""
    name = "_PrimitiveCreator__manage_color_attributes"
    try:
        from io_scene_gltf2.blender.exp import primitive_extract

        creator = primitive_extract.PrimitiveCreator
    except (ImportError, AttributeError):
        creator = None
    write_colors = getattr(creator, name, None)
    if write_colors is None:
        meshes = {obj.data.name: obj.data for obj in objects if obj.type == "MESH"}
        wrong = sorted(
            mesh_name
            for mesh_name, mesh in meshes.items()
            if mesh_data.DATA_COLOR in mesh.attributes and len(mesh.materials) > 1
        )
        if wrong:
            warnings.append(
                f"this Blender's glTF exporter has no {name} for the add-on to patch, so it may write the data "
                f"color of {', '.join(wrong)} white past each mesh's first material (ambient occlusion 1, full "
                "outline width and Z offset): export with Blender 5.2, which the add-on is tested on"
            )
        yield
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

    setattr(creator, name, fixed)
    try:
        yield
    finally:
        setattr(creator, name, write_colors)


@contextlib.contextmanager
def marked_scenes():
    """Every scene's custom property `sapphire_addon` (`MARK`) set to the add-on's version for the block's
    length, so the exporter writes it into the glTF scene's extras; put back after."""
    kept = []
    try:
        for scene in bpy.data.scenes:
            kept.append((scene, scene.get(MARK)))
            scene[MARK] = VERSION
        yield
    finally:
        for scene, value in kept:
            if value is None:
                scene.pop(MARK, None)
            else:
                scene[MARK] = value


@contextlib.contextmanager
def hull_previews_off():
    """Every hull preview modifier off for the block's length."""
    kept = []
    try:
        for obj in bpy.data.objects:
            modifier = mesh_data.hull_modifier(obj)
            if modifier is not None:
                kept.append((modifier, modifier.show_viewport, modifier.show_render))
                modifier.show_viewport = modifier.show_render = False
        yield
    finally:
        for modifier, viewport, render in kept:
            modifier.show_viewport, modifier.show_render = viewport, render


def _writing(filepath):
    """The name a file is written under before it takes its own: hidden, beside it, and still a `.glb` for the
    exporter. A cook skips hidden names, so a watch of the tree never cooks half a file."""
    folder, name = os.path.split(filepath)
    return os.path.join(folder, f".{name}.writing.glb")


def export_glb(filepath, *, selected_only=False, animations=False, images=False, apply_modifiers=False):
    """Exports the scene (or the selection) to `filepath` with the engine's settings, and returns the path and
    the export's warnings. The file is written under a hidden name and renamed into place, so whoever reads
    it (`cargo xtask content --watch`) sees it whole."""
    filepath = bpy.path.abspath(filepath)
    if not filepath.lower().endswith(".glb"):
        raise ValueError(f"{filepath}: the export writes a .glb")
    os.makedirs(os.path.dirname(filepath) or ".", exist_ok=True)
    writing = _writing(filepath)
    objects = exported_objects(selected_only)
    warnings = []
    try:
        with (
            at_export_frame(),
            marked_scenes(),
            every_materials_data_color(objects, warnings),
            outline_normals_in_gltf_space(objects),
            hull_previews_off(),
        ):
            result = bpy.ops.export_scene.gltf(
                filepath=writing,
                use_selection=selected_only,
                export_animations=animations,
                export_image_format="AUTO" if images else "NONE",
                export_apply=apply_modifiers,
                **ENGINE_SETTINGS,
            )
        if result != {"FINISHED"}:
            raise RuntimeError(f"Blender's glTF exporter didn't finish: {result}")
        os.replace(writing, filepath)
    finally:
        if os.path.exists(writing):
            os.remove(writing)
    return filepath, warnings


def inside(path, folder):
    """Whether `path` is in the tree at `folder`, both as Blender's paths (`//` relative to the .blend)."""
    path, folder = (os.path.realpath(bpy.path.abspath(p)) for p in (path, folder))
    return os.path.commonpath([path, folder]) == folder


def send_glb(filepath, content_root, **options):
    """Send: exports to `filepath`, which must be in the content tree at `content_root`, where a running
    `cargo xtask content --watch` cooks it and the running app reloads it."""
    if not content_root:
        raise ValueError("choose the Content folder: the tree `cargo xtask content --watch` cooks")
    if not inside(filepath, content_root):
        raise ValueError(f"{bpy.path.abspath(filepath)} isn't in the content tree {bpy.path.abspath(content_root)}")
    return export_glb(filepath, **options)


class SAPPHIRE_OT_export(bpy.types.Operator):
    """Export the scene as a .glb with the anime data Sapphire's importer reads"""

    bl_idname = "sapphire.export"
    bl_label = "Export for Sapphire"

    filepath: StringProperty(
        name="File",
        description="The .glb to write (default: the scene's export file)",
        subtype="FILE_PATH",
        default="",
    )

    def execute(self, context):
        return _run(self, context, lambda path, options: export_glb(path, **options), "wrote {}")


class SAPPHIRE_OT_send(bpy.types.Operator):
    """Export into the content tree, where `cargo xtask content --watch` cooks it and the running app
    reloads it"""

    bl_idname = "sapphire.send"
    bl_label = "Send to Sapphire"

    filepath: StringProperty(
        name="File",
        description="The .glb to write, in the content tree (default: the scene's export file)",
        subtype="FILE_PATH",
        default="",
    )

    def execute(self, context):
        root = context.scene.sapphire.content_root

        def send(path, options):
            return send_glb(path, root, **options)

        return _run(self, context, send, "sent {}: a running `cargo xtask content --watch` cooks it")


def _run(operator, context, write, done):
    """Runs `write(path, options)` for an operator with the scene's export settings, and reports the export's
    warnings and `done`."""
    settings = context.scene.sapphire
    path = operator.filepath or settings.export_path
    if not path:
        operator.report({"ERROR"}, "choose the .glb to export to")
        return {"CANCELLED"}
    options = {
        "selected_only": settings.selected_only,
        "animations": settings.animations,
        "images": settings.images,
        "apply_modifiers": settings.apply_modifiers,
    }
    try:
        written, warnings = write(path, options)
    except (ValueError, RuntimeError, mesh_data.MeshDataError) as error:
        operator.report({"ERROR"}, str(error))
        return {"CANCELLED"}
    for warning in warnings:
        operator.report({"WARNING"}, warning)
    settings.export_path = path
    operator.report({"INFO"}, done.format(written))
    return {"FINISHED"}


CLASSES = (SAPPHIRE_OT_export, SAPPHIRE_OT_send)

register, unregister = bpy.utils.register_classes_factory(CLASSES)
