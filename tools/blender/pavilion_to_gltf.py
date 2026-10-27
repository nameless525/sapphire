"""Converts VVayToyek's Chinese Four-corner Pavilion (CC0) to the glTF piece the look-dev town
places (M1 X1).

Run it headless; it reads the FBX and writes only the GLB (and a preview, if asked):

    blender -b --factory-startup --python tools/blender/pavilion_to_gltf.py -- \
        --fbx content/third_party/vvaytoyek_chinese_pavilion/chinese_four_corner_pavilion.fbx \
        --out content/lookdev/town/pieces/pavilion.glb

The FBX ships 70 meshes in 22 flat-colour materials with Chinese names, about 77,000
triangles, in units of roughly 15 cm. The piece is:

- **In meters**, scaled by --scale (0.15: the benches' seats stand 45 cm high, the bays between
  columns are 3.3 m, the finial tops out at 5.35 m), standing on its origin, +Y up, its front
  (the couplet boards and the name plaque) toward glTF +Z.
- **Six materials with ASCII names**, which the town's surface rules bind to its own environment
  materials: `lacquer` (columns, benches, handrails), `timber` (the frieze, architraves, cloud
  brackets, beams, purlins and rafters), `tiles` (the tile surface), `ridge` (hip ridges, the
  finial and the corner beasts), `stone` (the column bases) and `boards` (the blank couplet boards
  and plaque). The FBX's colours are dropped: the town draws its own.
- **Lighter**: the tile surface, hip ridges, corner beasts, finial, round rafters and cloud
  brackets are decimated by edge collapse (each by its own ratio, below). Most of their triangles
  were in smooth curves (along the slopes, round the rafters), so the roof keeps its tile rows and
  its sweep.
- **Without faces fighting in one plane**: the columns' tops and the upper corner beams' outer
  ends, flush with the architraves' tops and the hip ridges' ends, move 4 mm back (RECESS).
- **One mesh**, every part joined, with no texture coordinates (the town makes its own, and the
  pieces' coordinates would otherwise pick cells of the town's kit palette), colours, cameras,
  lights or animation.

It fails if two faces of different parts lie in one plane over a region (they would z-fight),
listing each pair, so a change of scale or decimation can't let one through unnoticed.
"""

import argparse
import math
import sys

import bmesh
import bpy
import mathutils

# Source material -> piece material.
MATERIALS = {
    "檐柱": "lacquer",  # eave columns
    "坐凳": "lacquer",  # benches
    "扶手": "lacquer",  # handrails
    "檐柱2": "stone",  # the columns' stone bases
    "挂落": "timber",  # hanging lattice frieze
    "额枋": "timber",  # architrave
    "檐檩垫板": "timber",  # eave purlin and pad board
    "角云": "timber",  # corner cloud brackets
    "老角梁": "timber",  # main corner beams
    "仔角梁由戗": "timber",  # upper corner beams and hip struts
    "抹角梁": "timber",  # diagonal corner beam
    "金檩金枋": "timber",  # upper purlin and tie beam
    "檐椽": "timber",  # eave rafters
    "飞椽": "timber",  # flying rafters
    "脑椽": "timber",  # top rafters
    "衬头木": "timber",  # rafter-raising blocks
    "瓦面": "tiles",  # tile surface
    "垂脊": "ridge",  # hip ridges
    "套兽": "ridge",  # beast caps on the corner beams
    "雷公柱": "ridge",  # king post and finial
    "对联": "boards",  # couplet boards
    "牌匾": "boards",  # name plaque
}

# Object name prefix -> edge-collapse ratio, for the parts whose triangles are mostly smooth
# curves.
DECIMATE = {
    "瓦面": 0.25,  # tile surface: 7,000 triangles a side
    "垂脊": 0.2,  # hip ridges: 4,314 each
    "套兽": 0.2,  # corner beasts: 1,108 each
    "雷公柱宝顶": 0.35,  # king post and finial: 1,438
    "檐椽": 0.35,  # round eave rafters: 1,800 a side
    "脑椽": 0.4,  # top rafters, under the roof: 540 a side
    "角云": 0.6,  # carved cloud brackets: 556 each
}

# Two faces fight if their normals are within this angle, their planes within this distance
# (meters) and they overlap over a region wider than this (meters): the cook's own check.
FIGHT_ANGLE = math.radians(2.5)
FIGHT_DISTANCE = 0.001
FIGHT_WIDTH = 0.002

# Parts whose faces in one plane with another part's are moved back by RECESS_DEPTH meters: the
# columns' tops, flush with the architraves' tops under the cloud brackets, and the upper corner
# beams' outer ends, flush with the hip ridges' ends at the eave corners.
RECESS = ("檐柱", "仔角梁由戗")
RECESS_DEPTH = 0.004


def parse_arguments():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(prog="pavilion_to_gltf.py", description=__doc__.splitlines()[0])
    parser.add_argument("--fbx", required=True, help="the pavilion's FBX")
    parser.add_argument("--out", required=True, help="the GLB to write")
    parser.add_argument("--scale", type=float, default=0.15, help="meters per FBX unit")
    parser.add_argument("--preview", default="", help="also render a workbench preview PNG here")
    return parser.parse_args(argv)


def source_material(name):
    # Blender suffixes duplicates (.001); the FBX's names are otherwise as listed.
    return name.split(".")[0]


def convert(arguments):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.fbx(filepath=arguments.fbx)
    scene = bpy.context.scene
    meshes = [ob for ob in scene.objects if ob.type == "MESH"]
    for ob in [ob for ob in scene.objects if ob.type != "MESH"]:
        bpy.data.objects.remove(ob)

    # One material per piece material, the FBX's slots remapped onto them.
    pieces = {name: bpy.data.materials.new(name) for name in sorted(set(MATERIALS.values()))}
    for ob in meshes:
        for slot in ob.material_slots:
            source = source_material(slot.material.name)
            if source not in MATERIALS:
                raise SystemExit(f"{ob.name}: material {source} has no piece material")
            slot.material = pieces[MATERIALS[source]]
        for layer in list(ob.data.uv_layers):
            ob.data.uv_layers.remove(layer)

    triangles_before = count_triangles(meshes)
    for ob in meshes:
        ratio = next((ratio for prefix, ratio in DECIMATE.items() if ob.name.startswith(prefix)), None)
        if ratio is not None:
            modifier = ob.modifiers.new("decimate", "DECIMATE")
            modifier.decimate_type = "COLLAPSE"
            modifier.ratio = ratio
            modifier.use_collapse_triangulate = True
            bpy.context.view_layer.objects.active = ob
            bpy.ops.object.modifier_apply(modifier=modifier.name)

    # Into meters, transforms applied, and joined.
    for ob in meshes:
        ob.matrix_world = mathutils.Matrix.Scale(arguments.scale, 4) @ ob.matrix_world
    bpy.ops.object.select_all(action="DESELECT")
    for ob in meshes:
        ob.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    parts = {ob.name: ob for ob in meshes}
    fights = recess(parts, find_fights(parts))
    fights += find_fights(parts)
    if fights:
        for fight in sorted(set(fights)):
            print("FIGHT", describe(parts, fight))
        raise SystemExit(f"{len(fights)} pairs of faces fight in one plane")
    bpy.ops.object.join()
    pavilion = bpy.context.view_layer.objects.active
    pavilion.name = "pavilion"
    pavilion.data.name = "pavilion"
    triangles_after = count_triangles([pavilion])

    bpy.ops.export_scene.gltf(
        filepath=arguments.out,
        export_format="GLB",
        use_selection=False,
        export_yup=True,
        export_apply=True,
        export_texcoords=False,
        export_normals=True,
        export_tangents=False,
        export_materials="EXPORT",
        export_cameras=False,
        export_lights=False,
        export_extras=False,
        export_animations=False,
        export_skins=False,
        export_morph=False,
    )
    lo, hi = bounds(pavilion)
    print(f"PAVILION {triangles_before} -> {triangles_after} triangles, bounds {lo} .. {hi} (Blender, Z up)")
    if arguments.preview:
        preview(arguments.preview, pieces)


def count_triangles(objects):
    total = 0
    for ob in objects:
        total += sum(len(polygon.vertices) - 2 for polygon in ob.data.polygons)
    return total


def bounds(ob):
    corners = [ob.matrix_world @ vertex.co for vertex in ob.data.vertices]
    lo = [round(min(c[i] for c in corners), 3) for i in range(3)]
    hi = [round(max(c[i] for c in corners), 3) for i in range(3)]
    return lo, hi


def find_fights(parts):
    """Pairs of triangles from different parts facing the same way in one plane, overlapping,
    that would draw differently: in different materials, or in one with different normals."""
    triangles = []
    for name, ob in parts.items():
        mesh = ob.data
        mesh.calc_loop_triangles()
        for triangle in mesh.loop_triangles:
            corners = [ob.matrix_world @ mesh.vertices[index].co for index in triangle.vertices]
            normal = (corners[1] - corners[0]).cross(corners[2] - corners[0])
            if normal.length < 1e-12:
                continue
            normal.normalize()
            material = mesh.materials[triangle.material_index].name
            flat = all(mathutils.Vector(n).angle(normal, 0.0) < FIGHT_ANGLE for n in triangle.split_normals)
            plane = normal.dot(corners[0])
            triangles.append((name, corners, normal, plane, material, flat, triangle.polygon_index))
    # Buckets by normal and plane; neighbors are searched too.
    step_normal = 0.05
    step_plane = 0.002
    buckets = {}
    for index, triangle in enumerate(triangles):
        normal, plane = triangle[2], triangle[3]
        key = (*(round(c / step_normal) for c in normal), round(plane / step_plane))
        buckets.setdefault(key, []).append(index)
    fights = set()
    for key, members in buckets.items():
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                for dz in (-1, 0, 1):
                    for dp in (-1, 0, 1):
                        other = buckets.get((key[0] + dx, key[1] + dy, key[2] + dz, key[3] + dp))
                        if not other:
                            continue
                        for a in members:
                            for b in other:
                                first, second = triangles[a], triangles[b]
                                if a < b and fight(first, second):
                                    fights.add((first[0], first[6], second[0], second[6]))
    return sorted(fights)


def describe(parts, fight):
    """A fight as its parts, their materials and where (meters, Blender's Z up)."""
    name_a, polygon_a, name_b, polygon_b = fight
    a, b = parts[name_a], parts[name_b]
    center = a.matrix_world @ a.data.polygons[polygon_a].center
    material = lambda ob, polygon: ob.data.materials[ob.data.polygons[polygon].material_index].name
    at = tuple(round(c, 3) for c in center)
    return f"{name_a} ({material(a, polygon_a)}) and {name_b} ({material(b, polygon_b)}) at {at}"


def recess(parts, fights):
    """Moves the fighting faces of the parts in RECESS back along their normals; the fights
    left are those between other parts."""
    moved = {}
    left = []
    for fight in fights:
        name_a, polygon_a, name_b, polygon_b = fight
        if any(name_a.startswith(prefix) for prefix in RECESS):
            moved.setdefault(name_a, set()).add(polygon_a)
        elif any(name_b.startswith(prefix) for prefix in RECESS):
            moved.setdefault(name_b, set()).add(polygon_b)
        else:
            left.append(fight)
    for name, polygons in moved.items():
        mesh = parts[name].data
        offsets = {}
        for index in polygons:
            polygon = mesh.polygons[index]
            for vertex in polygon.vertices:
                offsets.setdefault(vertex, mathutils.Vector())
                offsets[vertex] -= RECESS_DEPTH * polygon.normal
        for vertex, offset in offsets.items():
            # A corner shared by two moved faces moves back from each once.
            mesh.vertices[vertex].co += offset
        mesh.update()
    return left


def fight(first, second):
    name_a, corners_a, normal_a, plane_a, material_a, flat_a, _ = first
    name_b, corners_b, normal_b, plane_b, material_b, flat_b, _ = second
    if name_a == name_b or (material_a == material_b and flat_a and flat_b):
        return False
    if normal_a.angle(normal_b, 0.0) > FIGHT_ANGLE or abs(plane_a - plane_b) > FIGHT_DISTANCE:
        return False
    # In the first triangle's plane: clip the second by the first and measure what's left.
    u = (corners_a[1] - corners_a[0]).normalized()
    v = normal_a.cross(u)
    flat = lambda corners: [((c - corners_a[0]).dot(u), (c - corners_a[0]).dot(v)) for c in corners]
    polygon = flat(corners_b)
    clip = flat(corners_a)
    for i in range(3):
        (ax, ay), (bx, by) = clip[i], clip[(i + 1) % 3]
        inside = lambda p: (bx - ax) * (p[1] - ay) - (by - ay) * (p[0] - ax) >= 0.0
        result = []
        for j in range(len(polygon)):
            p, q = polygon[j], polygon[(j + 1) % len(polygon)]
            if inside(p):
                result.append(p)
            if inside(p) != inside(q):
                dx, dy = q[0] - p[0], q[1] - p[1]
                denominator = (bx - ax) * dy - (by - ay) * dx
                t = ((ax - p[0]) * (by - ay) - (ay - p[1]) * (bx - ax)) / -denominator if denominator else 0.0
                t = min(max(t, 0.0), 1.0)
                result.append((p[0] + t * dx, p[1] + t * dy))
        polygon = result
        if len(polygon) < 3:
            return False
    area = 0.0
    perimeter = 0.0
    for j in range(len(polygon)):
        (px, py), (qx, qy) = polygon[j], polygon[(j + 1) % len(polygon)]
        area += px * qy - qx * py
        perimeter += math.hypot(qx - px, qy - py)
    area = abs(area) / 2.0
    # Twice the area over the perimeter: the width of the overlap's thinnest part, roughly.
    return perimeter > 0.0 and 2.0 * area / perimeter > FIGHT_WIDTH / 2.0


def preview(path, pieces):
    colors = {
        "lacquer": (0.35, 0.06, 0.04, 1.0),
        "timber": (0.45, 0.24, 0.12, 1.0),
        "tiles": (0.08, 0.09, 0.1, 1.0),
        "ridge": (0.05, 0.05, 0.06, 1.0),
        "stone": (0.5, 0.5, 0.52, 1.0),
        "boards": (0.2, 0.12, 0.08, 1.0),
    }
    for name, material in pieces.items():
        material.diffuse_color = colors[name]
    scene = bpy.context.scene
    scene.render.engine = "BLENDER_WORKBENCH"
    scene.display.shading.light = "STUDIO"
    scene.display.shading.color_type = "MATERIAL"
    scene.display.shading.show_cavity = True
    scene.render.resolution_x = 960
    scene.render.resolution_y = 720
    scene.world = bpy.data.worlds.new("preview")
    camera = bpy.data.objects.new("camera", bpy.data.cameras.new("camera"))
    camera.data.lens = 35
    scene.collection.objects.link(camera)
    scene.camera = camera
    target = mathutils.Vector((0.0, 0.0, 2.4))
    camera.location = mathutils.Vector((8.7, -10.5, 4.5))
    camera.rotation_euler = (target - camera.location).to_track_quat("-Z", "Y").to_euler()
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)


if __name__ == "__main__":
    convert(parse_arguments())
