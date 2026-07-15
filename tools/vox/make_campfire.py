"""Generate assets/vox/campfire.vox — a small campfire prop.

A deterministic little voxel sculpture: ring of stones, crossed logs with
charred inner ends, and a jagged two-tone flame. Written as a standard
MagicaVoxel .vox file (Z-up), so it can be opened and repainted in
MagicaVoxel, and loads in the sandbox via:

    VOXEL_PROP=assets/vox/campfire.vox cargo run -p voxel-sandbox

Run:  python3 tools/vox/make_campfire.py
"""

import math
import random
import struct
from pathlib import Path

SIZE_X, SIZE_Y, SIZE_Z = 20, 20, 14
CENTER = (SIZE_X / 2 - 0.5, SIZE_Y / 2 - 0.5)

# Palette (index 1..n in file order).
COLORS = {
    "stone_light": (150, 148, 142, 255),
    "stone_dark": (117, 115, 110, 255),
    "log_brown": (112, 74, 44, 255),
    "log_dark": (84, 54, 32, 255),
    "char_black": (38, 32, 30, 255),
    "flame_red": (214, 64, 22, 255),
    "flame_orange": (243, 138, 26, 255),
    "flame_yellow": (252, 211, 74, 255),
}
COLOR_INDEX = {name: index + 1 for index, name in enumerate(COLORS)}

voxels = {}


def put(x, y, z, color_name):
    if 0 <= x < SIZE_X and 0 <= y < SIZE_Y and 0 <= z < SIZE_Z:
        voxels[(int(x), int(y), int(z))] = COLOR_INDEX[color_name]


random.seed(11)

# --- Stone ring --------------------------------------------------------------
for stone_index in range(9):
    angle = stone_index / 9 * math.tau + random.uniform(-0.12, 0.12)
    radius = 7.2 + random.uniform(-0.4, 0.4)
    stone_x = CENTER[0] + math.cos(angle) * radius
    stone_y = CENTER[1] + math.sin(angle) * radius
    color = "stone_light" if stone_index % 2 == 0 else "stone_dark"
    for dx in range(-1, 2):
        for dy in range(-1, 2):
            for dz in range(0, 2):
                if abs(dx) + abs(dy) + dz <= 2 + random.randint(0, 1):
                    put(stone_x + dx, stone_y + dy, dz, color)

# --- Crossed logs ------------------------------------------------------------
for log_index in range(4):
    angle = log_index / 4 * math.tau + 0.4
    direction = (math.cos(angle), math.sin(angle))
    for step in range(2, 7):
        log_x = CENTER[0] + direction[0] * step
        log_y = CENTER[1] + direction[1] * step
        # Inner ends are charred; logs slope up slightly toward the center.
        color = "char_black" if step < 4 else ("log_brown" if log_index % 2 == 0 else "log_dark")
        height = 1 if step < 4 else 0
        for dx in range(2):
            for dy in range(2):
                put(log_x + dx - 0.5, log_y + dy - 0.5, height, color)
                if step >= 5:
                    put(log_x + dx - 0.5, log_y + dy - 0.5, height + 1, color)

# --- Flame -------------------------------------------------------------------
for z in range(1, 8):
    flame_radius = max(0.5, 3.9 - z * 0.55)
    for dx in range(-4, 5):
        for dy in range(-4, 5):
            distance = math.hypot(dx, dy)
            jitter = random.uniform(-0.55, 0.55)
            if distance + jitter > flame_radius:
                continue
            if distance < flame_radius * 0.45:
                color = "flame_yellow"
            elif distance < flame_radius * 0.8:
                color = "flame_orange"
            else:
                color = "flame_red"
            put(CENTER[0] + dx, CENTER[1] + dy, z, color)

# Detached embers above the flame tip.
for _ in range(4):
    put(
        CENTER[0] + random.randint(-2, 2),
        CENTER[1] + random.randint(-2, 2),
        8 + random.randint(0, 2),
        "flame_orange",
    )

# --- Write .vox --------------------------------------------------------------
def chunk(chunk_id, content, children=b""):
    return chunk_id + struct.pack("<ii", len(content), len(children)) + content + children


size_chunk = chunk(b"SIZE", struct.pack("<iii", SIZE_X, SIZE_Y, SIZE_Z))
xyzi_body = struct.pack("<i", len(voxels)) + b"".join(
    struct.pack("<4B", x, y, z, color) for (x, y, z), color in sorted(voxels.items())
)
xyzi_chunk = chunk(b"XYZI", xyzi_body)

palette = [(0, 0, 0, 0)] * 256
for name, rgba in COLORS.items():
    palette[COLOR_INDEX[name] - 1] = rgba
rgba_chunk = chunk(b"RGBA", b"".join(struct.pack("<4B", *color) for color in palette))

main_chunk = chunk(b"MAIN", b"", size_chunk + xyzi_chunk + rgba_chunk)
output_path = Path(__file__).resolve().parents[2] / "assets" / "vox" / "campfire.vox"
output_path.parent.mkdir(parents=True, exist_ok=True)
output_path.write_bytes(b"VOX " + struct.pack("<i", 150) + main_chunk)
print(f"wrote {output_path} ({len(voxels)} voxels)")
