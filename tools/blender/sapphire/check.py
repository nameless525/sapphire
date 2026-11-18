# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""Check: the pipeline's `check` on the content tree the export went into, its numbers and findings kept
on the scene for the panel (`Scene.sapphire.measured`, `.findings`) and printed to the console.

The tree is the scene's content folder, or the exported file's folder. A source is judged at a role only
when its sidecar names one (`budget: (role: Hero)`), a line a person adds; a tree whose sources have no
sidecars can't be checked until a cook of it (`sapphire_pipeline cook`) writes them."""

import os

import bpy

from . import pipeline


def run(context, root=None):
    """Checks `root` (default: the scene's content folder or export folder), records the report on the
    scene and returns it."""
    settings = context.scene.sapphire
    if root is None:
        root = settings.content_root or os.path.dirname(settings.export_path)
    if not root:
        raise pipeline.PipelineError("export first, or choose the content folder to check")
    root = bpy.path.abspath(root)
    report = pipeline.check(pipeline.binary(context), root)
    settings.measured.clear()
    for mesh in report["meshes"]:
        for cook in mesh["cooks"]:
            row = settings.measured.add()
            row.source, row.role, row.cook = mesh["source"], mesh["role"], cook["cook"]
            for field in ("triangles", "joints", "slots", "map"):
                setattr(row, field, cook[field])
                setattr(row, f"{field}_limit", cook["limits"][field])
    settings.findings.clear()
    for finding in report["findings"]:
        row = settings.findings.add()
        row.source, row.rule, row.severity = finding["source"], finding["rule"], finding["severity"]
        row.cook = finding["cook"] or ""
        row.message = finding["message"]
        row.waived = finding["waived"] or ""
    lines = pipeline.describe(report)
    settings.check_summary = lines[-1]
    for line in lines:
        print(line)
    return report


class SAPPHIRE_OT_check(bpy.types.Operator):
    """Run the pipeline's check on the content tree: budgets at each role, and the content rules"""

    bl_idname = "sapphire.check"
    bl_label = "Check"

    def execute(self, context):
        try:
            report = run(context)
        except pipeline.PipelineError as error:
            self.report({"ERROR"}, str(error))
            return {"CANCELLED"}
        self.report({"INFO"} if pipeline.passes(report) else {"WARNING"}, context.scene.sapphire.check_summary)
        return {"FINISHED"}


CLASSES = (SAPPHIRE_OT_check,)

register, unregister = bpy.utils.register_classes_factory(CLASSES)
