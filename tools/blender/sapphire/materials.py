# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""The material extras Sapphire's glTF importer reads into a slot's generated anime character material
(`tools/sapphire_pipeline/src/import/gltf.rs`, `MaterialExtras`), kept as the material's custom
properties, which the export writes as its glTF extras:

- `shade_color`, `outline_color`: region 0's, linear RGB.
- `character_shading`: `[light-split threshold, softness, rim strength, outline width]`.
- `rim`: any of the depth-offset rim's `width`, `strength`, `threshold`, `fresnel`, `light_gate` and
  `back_light`.
- `rim_regions`: `[[strength, width, overlap], ...]` scales, one per region from region 0, which
  `_MATERIAL_ID` picks.

Back-face culling is the material's own setting: off, the slot is two-sided. A mesh's sidecar may bind a
material document to a slot instead, and then the document wins."""

import bpy
from bpy.props import EnumProperty, FloatProperty, FloatVectorProperty, IntProperty

# The defaults "Add" writes: the engine's own (`AnimeCharacterMaterial::default`).
DEFAULTS = {
    "shade_color": (0.55, 0.5, 0.65),
    "outline_color": (0.08, 0.06, 0.1),
    "character_shading": (0.0, 0.02, 0.35, 5.5),
}
CHARACTER_SHADING = ("Light split", "Softness", "Rim strength", "Outline width")
RIM_FIELDS = {
    "width": 4.0,
    "strength": 0.35,
    "threshold": 0.1,
    "fresnel": 0.5,
    "light_gate": 0.75,
    "back_light": 0.0,
}
REGIONS = 8


class ExtrasError(Exception):
    """A value the importer would refuse."""


def _numbers(key, value, count):
    numbers = [float(number) for number in value]
    if len(numbers) != count:
        raise ExtrasError(f"{key} takes {count} numbers, not {len(numbers)}")
    return numbers


def set_extras(material, *, shade_color=None, outline_color=None, character_shading=None, rim=None, rim_regions=None):
    """Writes the extras given (leaving the others as they are), checked as the importer reads them."""
    if shade_color is not None:
        _color(material, "shade_color", _numbers("shade_color", shade_color, 3))
    if outline_color is not None:
        _color(material, "outline_color", _numbers("outline_color", outline_color, 3))
    if character_shading is not None:
        material["character_shading"] = _numbers("character_shading", character_shading, 4)
    if rim is not None:
        unknown = set(rim) - set(RIM_FIELDS)
        if unknown:
            raise ExtrasError(f"rim has no {', '.join(sorted(unknown))}: its fields are {', '.join(RIM_FIELDS)}")
        material["rim"] = {key: float(value) for key, value in rim.items()}
    if rim_regions is not None:
        regions = [_numbers("a rim region", region, 3) for region in rim_regions]
        if not 0 < len(regions) <= REGIONS:
            raise ExtrasError(f"rim_regions takes 1 to {REGIONS} regions, not {len(regions)}")
        material["rim_regions"] = regions


def _color(material, key, value):
    material[key] = value
    material.id_properties_ui(key).update(subtype="COLOR", min=0.0, soft_max=1.0)


def remove_extra(material, key):
    if key in material:
        del material[key]


def extras(material):
    """The extras `material` holds, as the export writes them."""
    keys = ("shade_color", "outline_color", "character_shading", "rim", "rim_regions")
    found = {}
    for key in keys:
        if key not in material:
            continue
        value = material[key]
        if hasattr(value, "to_dict"):
            value = value.to_dict()
        elif hasattr(value, "to_list"):
            value = value.to_list()
        else:
            value = [list(region) for region in value]
        found[key] = value
    return found


class SAPPHIRE_OT_extra_add(bpy.types.Operator):
    """Add this extra to the material, at the engine's default"""

    bl_idname = "sapphire.extra_add"
    bl_label = "Add Extra"
    bl_options = {"REGISTER", "UNDO"}

    key: EnumProperty(
        items=(
            ("shade_color", "Shade color", ""),
            ("outline_color", "Outline color", ""),
            ("character_shading", "Character shading", ""),
        )
    )

    @classmethod
    def poll(cls, context):
        return context.material is not None

    def execute(self, context):
        set_extras(context.material, **{self.key: DEFAULTS[self.key]})
        return {"FINISHED"}


class SAPPHIRE_OT_extra_remove(bpy.types.Operator):
    """Remove this extra: the generated material keeps the engine's default"""

    bl_idname = "sapphire.extra_remove"
    bl_label = "Remove Extra"
    bl_options = {"REGISTER", "UNDO"}

    key: bpy.props.StringProperty()

    @classmethod
    def poll(cls, context):
        return context.material is not None

    def execute(self, context):
        remove_extra(context.material, self.key)
        return {"FINISHED"}


class SAPPHIRE_OT_rim_edit(bpy.types.Operator):
    """Set the depth-offset rim's settings (every field is written)"""

    bl_idname = "sapphire.rim_edit"
    bl_label = "Edit Rim"
    bl_options = {"REGISTER", "UNDO"}

    width: FloatProperty(name="Width", min=0.0, default=RIM_FIELDS["width"])
    strength: FloatProperty(name="Strength", min=0.0, default=RIM_FIELDS["strength"])
    threshold: FloatProperty(name="Threshold", min=0.0, default=RIM_FIELDS["threshold"])
    fresnel: FloatProperty(name="Fresnel", min=0.0, max=1.0, default=RIM_FIELDS["fresnel"])
    light_gate: FloatProperty(name="Light gate", min=0.0, max=1.0, default=RIM_FIELDS["light_gate"])
    back_light: FloatProperty(name="Back light", min=0.0, max=1.0, default=RIM_FIELDS["back_light"])

    @classmethod
    def poll(cls, context):
        return context.material is not None

    def invoke(self, context, _event):
        current = extras(context.material).get("rim", {})
        for field, default in RIM_FIELDS.items():
            setattr(self, field, current.get(field, default))
        return context.window_manager.invoke_props_dialog(self)

    def execute(self, context):
        set_extras(context.material, rim={field: getattr(self, field) for field in RIM_FIELDS})
        return {"FINISHED"}


class SAPPHIRE_OT_rim_region_set(bpy.types.Operator):
    """Set one region's rim scales; regions before it that are missing get neutral scales (1, 1, 1)"""

    bl_idname = "sapphire.rim_region_set"
    bl_label = "Set Rim Region"
    bl_options = {"REGISTER", "UNDO"}

    region: IntProperty(name="Region", min=0, max=REGIONS - 1)
    scales: FloatVectorProperty(name="Strength, width, overlap", size=3, min=0.0, default=(1.0, 1.0, 1.0))

    @classmethod
    def poll(cls, context):
        return context.material is not None

    def invoke(self, context, _event):
        regions = extras(context.material).get("rim_regions", [])
        if self.region < len(regions):
            self.scales = regions[self.region]
        return context.window_manager.invoke_props_dialog(self)

    def execute(self, context):
        regions = extras(context.material).get("rim_regions", [])
        while len(regions) <= self.region:
            regions.append([1.0, 1.0, 1.0])
        regions[self.region] = list(self.scales)
        set_extras(context.material, rim_regions=regions)
        return {"FINISHED"}


class SAPPHIRE_PT_material(bpy.types.Panel):
    """The material's anime character settings, in the material properties."""

    bl_label = "Sapphire"
    bl_space_type = "PROPERTIES"
    bl_region_type = "WINDOW"
    bl_context = "material"

    @classmethod
    def poll(cls, context):
        return context.material is not None

    def draw(self, context):
        layout = self.layout
        material = context.material
        layout.prop(material, "use_backface_culling", text="Back-face culling (off: two-sided)")
        principled = material.node_tree.nodes.get("Principled BSDF") if material.node_tree else None
        if principled is not None:
            layout.prop(principled.inputs["Base Color"], "default_value", text="Base color")
        for key, label in (("shade_color", "Shade color"), ("outline_color", "Outline color")):
            row = layout.row(align=True)
            if key in material:
                row.prop(material, f'["{key}"]', text=label)
                row.operator("sapphire.extra_remove", text="", icon="X").key = key
            else:
                row.operator("sapphire.extra_add", text=f"Add {label.lower()}").key = key
        if "character_shading" in material:
            box = layout.box()
            header = box.row()
            header.label(text="Character shading")
            header.operator("sapphire.extra_remove", text="", icon="X").key = "character_shading"
            for index, label in enumerate(CHARACTER_SHADING):
                box.prop(material, '["character_shading"]', index=index, text=label)
        else:
            layout.operator("sapphire.extra_add", text="Add character shading").key = "character_shading"
        values = extras(material)
        box = layout.box()
        header = box.row()
        header.label(text="Rim")
        header.operator("sapphire.rim_edit", text="", icon="GREASEPENCIL")
        if "rim" in values:
            header.operator("sapphire.extra_remove", text="", icon="X").key = "rim"
            for field, value in values["rim"].items():
                box.label(text=f"{field}: {value:.3g}")
        box = layout.box()
        header = box.row()
        header.label(text="Rim regions")
        regions = values.get("rim_regions", [])
        if regions:
            header.operator("sapphire.extra_remove", text="", icon="X").key = "rim_regions"
        for index, (strength, width, overlap) in enumerate(regions):
            row = box.row()
            row.label(text=f"{index}: strength {strength:.3g}, width {width:.3g}, overlap {overlap:.3g}")
            row.operator("sapphire.rim_region_set", text="", icon="GREASEPENCIL").region = index
        if len(regions) < REGIONS:
            box.operator("sapphire.rim_region_set", text="Add region", icon="ADD").region = len(regions)


CLASSES = (
    SAPPHIRE_OT_extra_add,
    SAPPHIRE_OT_extra_remove,
    SAPPHIRE_OT_rim_edit,
    SAPPHIRE_OT_rim_region_set,
    SAPPHIRE_PT_material,
)

register, unregister = bpy.utils.register_classes_factory(CLASSES)
