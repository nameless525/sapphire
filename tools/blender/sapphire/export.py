# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""The export: Blender's own glTF exporter, with the settings Sapphire's importer reads
(`ENGINE_SETTINGS`), and around it what Blender 5.2's exporter gets wrong for this data:

- It writes custom vector attributes as they are, where it turns positions and normals from Blender's
  Z-up into glTF's Y-up, so `_OUTLINE_NORMAL` is turned for the export's length and put back after
  (`outline_normals_in_gltf_space`).
- It writes a mesh's data color into its first material's primitive only, and every other material's
  corners white (`every_materials_data_color`).
- The hull preview's modifiers are switched off while it runs, so applying modifiers never exports them.

Send (M6 C4) is the export into the content tree: the file must be in the scene's content folder, where a
running `cargo xtask content --watch` cooks it, and the running app's hot reload or live link shows it. Every
export writes a hidden file and renames it into place, so a watch never cooks half a file.

Each run writes the same bytes for the same scene and Blender version: the exporter's output depends on
the scene alone, and the scene's add-on settings stay out of its extras (`settings`)."""

import contextlib
import os

import bpy
import numpy as np
from bpy.props import StringProperty

from . import mesh_data

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


@contextlib.contextmanager
def outline_normals_in_gltf_space():
    """Every mesh's `_OUTLINE_NORMAL` in glTF's space while the block runs, put back after."""
    kept = []
    try:
        for mesh in bpy.data.meshes:
            layer = mesh.attributes.get(mesh_data.OUTLINE_NORMAL)
            if layer is None or layer.data_type != "FLOAT_VECTOR":
                continue
            values = np.empty(len(layer.data) * 3, dtype=np.float32)
            layer.data.foreach_get("vector", values)
            kept.append((mesh, values))
            layer.data.foreach_set("vector", _z_up_to_y_up(values.reshape(-1, 3)).ravel())
            mesh.update()
        yield
    finally:
        for mesh, values in kept:
            mesh.attributes[mesh_data.OUTLINE_NORMAL].data.foreach_set("vector", values)
            mesh.update()


@contextlib.contextmanager
def every_materials_data_color():
    """Works around Blender 5.2's glTF exporter writing a mesh's vertex colors into its first material's
    primitive only: for every later material it records the color attribute's name where the glTF
    attribute's (`COLOR_0`) belongs, then writes those corners white. Puts the glTF attribute back before
    the colors are written, for the block's length; a no-op once the exporter changes."""
    from io_scene_gltf2.blender.exp import primitive_extract

    creator = primitive_extract.PrimitiveCreator
    name = "_PrimitiveCreator__manage_color_attributes"
    write_colors = getattr(creator, name, None)
    if write_colors is None:
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
    """Exports the scene (or the selection) to `filepath` with the engine's settings. The file is written under
    a hidden name and renamed into place, so whoever reads it (`cargo xtask content --watch`) sees it whole."""
    filepath = bpy.path.abspath(filepath)
    if not filepath.lower().endswith(".glb"):
        raise ValueError(f"{filepath}: the export writes a .glb")
    os.makedirs(os.path.dirname(filepath) or ".", exist_ok=True)
    writing = _writing(filepath)
    try:
        with every_materials_data_color(), outline_normals_in_gltf_space(), hull_previews_off():
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
    return filepath


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
    """Runs `write(path, options)` for an operator with the scene's export settings, and reports `done`."""
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
        written = write(path, options)
    except (ValueError, RuntimeError, mesh_data.MeshDataError) as error:
        operator.report({"ERROR"}, str(error))
        return {"CANCELLED"}
    settings.export_path = path
    operator.report({"INFO"}, done.format(written))
    return {"FINISHED"}


CLASSES = (SAPPHIRE_OT_export, SAPPHIRE_OT_send)

register, unregister = bpy.utils.register_classes_factory(CLASSES)
