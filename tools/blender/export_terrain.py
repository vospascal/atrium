"""Export a Blender terrain for the Atrium voxel sandbox.

Usage (inside Blender):
  1. Select your terrain object — any modifier / geometry-nodes stack works,
     the EVALUATED surface is what gets sampled.
  2. Run this script (Text editor ▸ Run, or paste in the Python console).
  3. Load it in voxel-rt: cargo run -p voxel-rt -- <OUTPUT>.terrain.json

Conventions (how the Blender layout maps into the engine):
  * Blender meters == engine meters; the object's XY bounding box is mapped
    onto the sandbox plateau, scene center → plateau center.
  * Blender Z = 0 is the WATER PLANE: surface below zero becomes riverbed /
    pond (auto-filled with water), above zero becomes land.
  * Where a downward ray misses the surface entirely → open sky beyond the
    plateau rim (NaN in the heightmap).
  * Trees: every depsgraph instance or object whose source name contains
    TREE_NAME_TAG is exported as a tree position (works with geometry-nodes
    scatters — no need to make instances real).

Output: <name>.terrain.json + <name>.heights.raw (little-endian f32 grid,
row-major, NaN = no surface).
"""

import json
import math
import struct

import bpy
from mathutils import Vector
from mathutils.bvhtree import BVHTree

# ---- knobs -----------------------------------------------------------------
RESOLUTION = 512          # heightmap samples per side
OUTPUT_STEM = ""          # empty → "<object name>" next to the .blend file
TREE_NAME_TAG = "tree"    # case-insensitive substring marking tree instances
# -----------------------------------------------------------------------------


def build_world_space_bvh(evaluated_object):
    mesh = evaluated_object.to_mesh()
    matrix = evaluated_object.matrix_world
    vertices = [matrix @ vertex.co for vertex in mesh.vertices]
    polygons = [tuple(polygon.vertices) for polygon in mesh.polygons]
    bvh = BVHTree.FromPolygons(vertices, polygons)
    evaluated_object.to_mesh_clear()
    return bvh, vertices


def export_terrain():
    terrain_object = bpy.context.active_object
    if terrain_object is None:
        raise RuntimeError("select the terrain object first")

    depsgraph = bpy.context.evaluated_depsgraph_get()
    evaluated = terrain_object.evaluated_get(depsgraph)
    bvh, vertices = build_world_space_bvh(evaluated)

    min_x = min(vertex.x for vertex in vertices)
    max_x = max(vertex.x for vertex in vertices)
    min_y = min(vertex.y for vertex in vertices)
    max_y = max(vertex.y for vertex in vertices)
    max_z = max(vertex.z for vertex in vertices)
    ray_start_height = max_z + 10.0
    ray_direction = Vector((0.0, 0.0, -1.0))

    heights = []
    for row in range(RESOLUTION):
        # Blender +Y ↔ engine +Z (row axis); X stays X.
        y = min_y + (max_y - min_y) * row / (RESOLUTION - 1)
        for column in range(RESOLUTION):
            x = min_x + (max_x - min_x) * column / (RESOLUTION - 1)
            hit = bvh.ray_cast(Vector((x, y, ray_start_height)), ray_direction)
            heights.append(hit[0].z if hit[0] is not None else math.nan)

    # Tree positions: depsgraph instances catch geometry-nodes scatters too.
    trees_uv = []
    span_x = max(max_x - min_x, 1e-6)
    span_y = max(max_y - min_y, 1e-6)
    for instance in depsgraph.object_instances:
        source_name = instance.object.name.lower()
        if TREE_NAME_TAG not in source_name:
            continue
        location = instance.matrix_world.translation
        u = (location.x - min_x) / span_x
        v = (location.y - min_y) / span_y
        if 0.0 <= u <= 1.0 and 0.0 <= v <= 1.0:
            trees_uv.append([round(u, 5), round(v, 5)])

    stem = OUTPUT_STEM or terrain_object.name.lower().replace(" ", "_")
    base_path = bpy.path.abspath("//") or bpy.app.tempdir
    heights_name = f"{stem}.heights.raw"

    with open(f"{base_path}{heights_name}", "wb") as raw_file:
        raw_file.write(struct.pack(f"<{len(heights)}f", *heights))

    meta = {
        "width": RESOLUTION,
        "depth": RESOLUTION,
        "heights_file": heights_name,
        "trees_uv": trees_uv or None,
    }
    json_path = f"{base_path}{stem}.terrain.json"
    with open(json_path, "w") as json_file:
        json.dump(meta, json_file)

    land_samples = sum(1 for height in heights if not math.isnan(height))
    print(
        f"exported {json_path}: {RESOLUTION}×{RESOLUTION} heightmap "
        f"({land_samples} land samples), {len(trees_uv)} trees"
    )
    return json_path


export_terrain()
