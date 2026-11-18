# SPDX-License-Identifier: MIT
# Copyright (c) 2026 nameless525
"""Runs Sapphire's pipeline binary, which holds every rule: the add-on checks nothing itself.

- Check: `sapphire_pipeline check <tree> --json`, C1's budgets and content rules on a source tree, as CI
  and the content tests apply them.
- The face sheet: `sapphire_pipeline bake-map <face.facesdf.ron> <png> --sheet <angles>`, the face map lit
  at those angles beside the masks, red where it misses them.

The binary is the add-on's preference, else `$SAPPHIRE_PIPELINE`, else `sapphire_pipeline` on the PATH."""

import json
import os
import shutil
import subprocess

import bpy

from . import settings

# How long a run may take: a check cooks each role mesh to memory on both cooks.
TIMEOUT = 600


class PipelineError(Exception):
    """The binary couldn't be found or run, or said something other than its report."""


def binary(context):
    """The pipeline binary to run."""
    preferences = settings.preferences(context)
    chosen = (preferences.pipeline if preferences is not None else "") or os.environ.get("SAPPHIRE_PIPELINE", "")
    if chosen:
        chosen = bpy.path.abspath(chosen)
        if not os.path.isfile(chosen):
            raise PipelineError(f"no pipeline binary at {chosen}")
        return chosen
    found = shutil.which("sapphire_pipeline")
    if found is None:
        raise PipelineError(
            "no sapphire_pipeline binary: build it (cargo build -p sapphire_pipeline --profile tools) and set "
            "it in the add-on's preferences or $SAPPHIRE_PIPELINE"
        )
    return found


def _run(arguments):
    try:
        return subprocess.run(arguments, capture_output=True, text=True, timeout=TIMEOUT, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PipelineError(f"running {arguments[0]}: {error}") from error


def check(pipeline, root):
    """`sapphire_pipeline check --json` on the tree at `root`: its report, a dict with `meshes`,
    `character_maps` and `findings`. A tree that fails the check still has its report; one the check
    can't read raises with the binary's own words."""
    finished = _run([pipeline, "check", root, "--json"])
    try:
        return json.loads(finished.stdout)
    except json.JSONDecodeError:
        words = (finished.stderr or finished.stdout).strip()
        raise PipelineError(words or f"the check exited with {finished.returncode} and said nothing") from None


def passes(report):
    """Whether a check's report passes: no error a waiver doesn't cover."""
    return not any(finding["severity"] == "error" and finding["waived"] is None for finding in report["findings"])


def describe(report):
    """The report as lines of text: each role mesh's numbers against its row, then each finding."""
    lines = []
    for mesh in report["meshes"]:
        for measured in mesh["cooks"]:
            limits = measured["limits"]
            lines.append(
                f"{mesh['source']} ({mesh['role']}, {measured['cook']}): "
                f"{measured['triangles']}/{limits['triangles']} triangles, {measured['joints']}/{limits['joints']} "
                f"joints, {measured['slots']}/{limits['slots']} slots, maps {measured['map']}/{limits['map']}"
            )
    for finding in report["findings"]:
        cook = f", {finding['cook']}" if finding["cook"] else ""
        waived = f" (waived: {finding['waived']})" if finding["waived"] is not None else ""
        lines.append(f"{finding['severity']} {finding['rule']}: {finding['source']}{cook}: {finding['message']}{waived}")
    verdict = "passes" if passes(report) else "fails"
    lines.append(f"the check {verdict}: {len(report['meshes'])} role mesh(es), {len(report['findings'])} finding(s)")
    return lines


def face_sheet(pipeline, document, output, angles):
    """Writes the face sheet of the face SDF document at `document` to `output` (a PNG)."""
    finished = _run([pipeline, "bake-map", document, output, "--sheet", ",".join(f"{angle:g}" for angle in angles)])
    if finished.returncode != 0:
        raise PipelineError((finished.stderr or finished.stdout).strip() or f"bake-map exited with {finished.returncode}")
    return output
