# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""The add-on's panels, in the 3D view's sidebar under "Sapphire", in the order an artist goes: export,
material IDs, the data color and the hull, outline normals, masks, then Check. The material extras are in
the material properties (`materials`). A new panel joins `PANELS`, in its place in that order."""

import bpy

from . import mesh_data


class _Sidebar:
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "Sapphire"


def _mesh(context):
    obj = context.active_object
    return obj.data if obj is not None and obj.type == "MESH" else None


class SAPPHIRE_PT_export(_Sidebar, bpy.types.Panel):
    bl_label = "Export"

    def draw(self, context):
        layout = self.layout
        settings = context.scene.sapphire
        layout.prop(settings, "export_path")
        row = layout.row()
        row.prop(settings, "selected_only")
        row.prop(settings, "animations")
        row = layout.row()
        row.prop(settings, "images")
        row.prop(settings, "apply_modifiers")
        layout.operator("sapphire.export", icon="EXPORT")
        # Send: into the content tree, which `cargo xtask content --watch` cooks on each change (M6 C4).
        layout.prop(settings, "content_root")
        layout.operator("sapphire.send", icon="FILE_REFRESH")


class SAPPHIRE_PT_material_ids(_Sidebar, bpy.types.Panel):
    """Paint `_MATERIAL_ID` on faces in edit mode."""

    bl_label = "Material IDs"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, context):
        layout = self.layout
        mesh = _mesh(context)
        if mesh is None:
            layout.label(text="Select a mesh")
            return
        layout.label(text="The region of its material a face shades with")
        grid = layout.grid_flow(columns=4, align=True)
        for material_id in range(mesh_data.MATERIAL_IDS):
            grid.operator("sapphire.material_id_assign", text=str(material_id)).material_id = material_id
        if context.mode == "EDIT_MESH":
            grid = layout.grid_flow(columns=4, align=True)
            for material_id in range(mesh_data.MATERIAL_IDS):
                grid.operator("sapphire.material_id_select", text=f"Select {material_id}").material_id = material_id
        else:
            layout.label(text="Object mode paints every face", icon="INFO")


class SAPPHIRE_PT_data_color(_Sidebar, bpy.types.Panel):
    """Set the data color's channels, and preview the hull they shape."""

    bl_label = "Data Color and Hull"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, context):
        layout = self.layout
        settings = context.scene.sapphire
        if _mesh(context) is None:
            layout.label(text="Select a mesh")
            return
        for channel, prop in (("AO", "ambient_occlusion"), ("WIDTH", "outline_width"), ("Z_OFFSET", "outline_z_offset")):
            row = layout.row(align=True)
            row.prop(settings, prop)
            operator = row.operator("sapphire.data_color_set", text="Set")
            operator.channel = channel
            operator.value = getattr(settings, prop)
        layout.label(text=f"Paint more in Vertex Paint on {mesh_data.DATA_COLOR}")
        row = layout.row(align=True)
        row.prop(settings, "hull_pixel")
        shown = mesh_data.hull_modifier(context.active_object) is not None
        row.operator("sapphire.hull_preview", text="Refresh" if shown else "Show hull", icon="MOD_SOLIDIFY").show = True
        if shown:
            row.operator("sapphire.hull_preview", text="", icon="X").show = False


class SAPPHIRE_PT_outline_normals(_Sidebar, bpy.types.Panel):
    bl_label = "Outline Normals"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, context):
        layout = self.layout
        mesh = _mesh(context)
        if mesh is None:
            layout.label(text="Select a mesh")
            return
        layout.prop(context.scene.sapphire, "outline_source")
        row = layout.row(align=True)
        row.operator("sapphire.outline_normals", icon="NORMALS_VERTEX")
        row.operator("sapphire.outline_normals_clear", text="", icon="X")
        state = "authored" if mesh_data.OUTLINE_NORMAL in mesh.attributes else "baked by the cook (welded)"
        layout.label(text=f"Outline normals: {state}")


class SAPPHIRE_PT_masks(_Sidebar, bpy.types.Panel):
    bl_label = "Masks"
    bl_options = {"DEFAULT_CLOSED"}

    def draw(self, _context):
        layout = self.layout
        layout.operator("sapphire.bake_face_masks", icon="RENDER_STILL")
        layout.operator("sapphire.bake_body_masks", icon="RENDER_STILL")
        layout.operator("sapphire.face_sheet", icon="IMAGE_DATA")


class SAPPHIRE_PT_check(_Sidebar, bpy.types.Panel):
    bl_label = "Check"

    def draw(self, context):
        layout = self.layout
        settings = context.scene.sapphire
        layout.prop(settings, "content_root")
        layout.operator("sapphire.check", icon="CHECKMARK")
        if settings.check_summary:
            layout.label(text=settings.check_summary)
        for row in settings.measured:
            box = layout.box()
            box.label(text=f"{row.source}: {row.role}, {row.cook}")
            for field in ("triangles", "joints", "slots", "map"):
                value, limit = getattr(row, field), getattr(row, f"{field}_limit")
                box.label(text=f"{field}: {value} of {limit}", icon="ERROR" if value > limit else "CHECKMARK")
        for finding in settings.findings:
            icon = "INFO" if finding.waived else ("ERROR" if finding.severity == "error" else "QUESTION")
            column = layout.column(align=True)
            column.label(text=f"{finding.rule}: {finding.source}" + (f" ({finding.cook})" if finding.cook else ""), icon=icon)
            column.label(text=finding.message)
            if finding.waived:
                column.label(text=f"waived: {finding.waived}")


PANELS = (
    SAPPHIRE_PT_export,
    SAPPHIRE_PT_material_ids,
    SAPPHIRE_PT_data_color,
    SAPPHIRE_PT_outline_normals,
    SAPPHIRE_PT_masks,
    SAPPHIRE_PT_check,
)

register, unregister = bpy.utils.register_classes_factory(PANELS)
