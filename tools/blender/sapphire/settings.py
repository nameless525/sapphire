# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""The add-on's settings: the pipeline binary (a preference), and each scene's export path, content
tree, outline normal source, hull preview and last check (`Scene.sapphire`).

The scene's settings are a registered property, which Blender 5.2's glTF exporter would write into
the scene's extras with the rest of its custom properties, local paths included: `register` adds
the name to the exporter's list of properties it never writes."""

import bpy
from bpy.props import (
    BoolProperty,
    CollectionProperty,
    FloatProperty,
    IntProperty,
    PointerProperty,
    StringProperty,
)

# The name of the scene's settings, which the glTF exporter must not write.
SCENE_SETTINGS = "sapphire"


class SapphirePreferences(bpy.types.AddonPreferences):
    bl_idname = __package__

    pipeline: StringProperty(
        name="Pipeline",
        description=(
            "The sapphire_pipeline binary Check and the face sheet run (cargo build -p sapphire_pipeline "
            "--profile tools writes it to target/tools/). Empty: $SAPPHIRE_PIPELINE, then the PATH"
        ),
        subtype="FILE_PATH",
        default="",
    )

    def draw(self, _context):
        self.layout.prop(self, "pipeline")


class SapphireFinding(bpy.types.PropertyGroup):
    """One finding of the last check, as `sapphire_pipeline check --json` reports it."""

    source: StringProperty()
    rule: StringProperty()
    severity: StringProperty()
    cook: StringProperty()
    message: StringProperty()
    waived: StringProperty(description="The waiver's reason, if the source's sidecar waives the rule")


class SapphireMeasured(bpy.types.PropertyGroup):
    """One role mesh on one cook, with its row, as the last check measured it."""

    source: StringProperty()
    role: StringProperty()
    cook: StringProperty()
    triangles: IntProperty()
    joints: IntProperty()
    slots: IntProperty()
    map: IntProperty()
    triangles_limit: IntProperty()
    joints_limit: IntProperty()
    slots_limit: IntProperty()
    map_limit: IntProperty()


class SapphireSettings(bpy.types.PropertyGroup):
    export_path: StringProperty(
        name="File",
        description="The .glb the export writes, inside the content tree with its .meta sidecar",
        subtype="FILE_PATH",
        default="",
    )
    selected_only: BoolProperty(name="Selected only", description="Export the selected objects alone")
    animations: BoolProperty(
        name="Animations",
        description="Export the scene's actions as glTF animations (the mesh's clips); off, the rest pose alone",
    )
    images: BoolProperty(
        name="Images",
        description=(
            "Embed the materials' images. Off by default: the engine binds a character's maps through "
            "material documents, so embedded images only grow the file"
        ),
    )
    apply_modifiers: BoolProperty(
        name="Apply modifiers",
        description=(
            "Apply modifiers other than the armature (Blender's exporter then drops shape keys); the hull "
            "preview is never exported"
        ),
    )
    content_root: StringProperty(
        name="Content",
        description=(
            "The content tree: where Send exports to, and what Check reads (default for Check: the exported "
            "file's folder)"
        ),
        subtype="DIR_PATH",
        default="",
    )
    material_id: IntProperty(name="ID", description="The material ID to paint", min=0, max=7, default=0)
    ambient_occlusion: FloatProperty(
        name="AO", description="Ambient occlusion (R): 1 unoccluded", min=0.0, max=1.0, default=1.0
    )
    outline_width: FloatProperty(
        name="Width", description="Outline width scale (G): 0 hides the line", min=0.0, max=1.0, default=1.0
    )
    outline_z_offset: FloatProperty(
        name="Z offset", description="Outline Z offset scale (B)", min=0.0, max=1.0, default=0.0
    )
    outline_source: PointerProperty(
        name="Source",
        description="Transfer outline normals from this mesh's smooth normals (empty: the mesh's own, welded)",
        type=bpy.types.Object,
        poll=lambda _self, obj: obj.type == "MESH",
    )
    hull_pixel: FloatProperty(
        name="Pixel",
        description=(
            "Meters a pixel of outline width spans in the preview: at the outline curve's mid distance (3 m), "
            "a 1080-row frame spans about 2.5 m, 2.3 mm a row"
        ),
        min=0.0,
        default=0.0023,
        precision=5,
        unit="LENGTH",
    )
    findings: CollectionProperty(type=SapphireFinding)
    measured: CollectionProperty(type=SapphireMeasured)
    check_summary: StringProperty(name="Last check")


CLASSES = (SapphirePreferences, SapphireFinding, SapphireMeasured, SapphireSettings)


def preferences(context):
    """The add-on's preferences, or None when it was imported rather than enabled (a headless script)."""
    addon = context.preferences.addons.get(__package__)
    return addon.preferences if addon is not None else None


def _extras_black_list():
    from io_scene_gltf2.blender.com import extras

    return extras.BLACK_LIST


def register():
    for cls in CLASSES:
        bpy.utils.register_class(cls)
    bpy.types.Scene.sapphire = PointerProperty(type=SapphireSettings)
    black_list = _extras_black_list()
    if SCENE_SETTINGS not in black_list:
        black_list.append(SCENE_SETTINGS)


def unregister():
    black_list = _extras_black_list()
    if SCENE_SETTINGS in black_list:
        black_list.remove(SCENE_SETTINGS)
    del bpy.types.Scene.sapphire
    for cls in reversed(CLASSES):
        bpy.utils.unregister_class(cls)
