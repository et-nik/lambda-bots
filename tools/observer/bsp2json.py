#!/usr/bin/env python3
"""Extract 2D wall geometry from a GoldSrc BSP (version 30) for the observer.

Takes the worldspawn faces whose plane is closer to vertical than horizontal
(|normal.z| < 0.7 - the walls), projects their edges to the XY plane and emits
them as line segments with a z-range, so the viewer can draw a floor plan and
fade geometry outside the current height window.

Usage:
    python3 bsp2json.py crossfire.bsp > crossfire.json
    python3 bsp2json.py --selftest

bridge.py imports `convert()` from here to serve /maps/<name>.json on the fly.
Only the Python standard library is used.
"""

import json
import math
import pathlib
import struct
import sys

BSP_VERSION = 30
LUMP_COUNT = 15

(LUMP_ENTITIES, LUMP_PLANES, LUMP_TEXTURES, LUMP_VERTICES, LUMP_VISIBILITY,
 LUMP_NODES, LUMP_TEXINFO, LUMP_FACES, LUMP_LIGHTING, LUMP_CLIPNODES,
 LUMP_LEAVES, LUMP_MARKSURFACES, LUMP_EDGES, LUMP_SURFEDGES, LUMP_MODELS) = range(LUMP_COUNT)

WALL_NORMAL_Z = 0.7  # |nz| below this -> face is a wall
MIN_SEGMENT_2D = 2.0  # skip edges that are (nearly) vertical lines in 2d


def _read_lumps(data):
    (version,) = struct.unpack_from("<i", data, 0)
    if version != BSP_VERSION:
        raise ValueError(f"unsupported BSP version {version} (want {BSP_VERSION})")
    lumps = []
    for i in range(LUMP_COUNT):
        offset, length = struct.unpack_from("<ii", data, 4 + i * 8)
        lumps.append(data[offset:offset + length])
    return lumps


def _parse_entities(raw):
    """Parse the entities text lump into a list of key->value dicts."""
    text = raw.split(b"\x00", 1)[0].decode("ascii", errors="replace")
    entities, current = [], None
    for line in text.splitlines():
        line = line.strip()
        if line == "{":
            current = {}
        elif line == "}":
            if current is not None:
                entities.append(current)
            current = None
        elif current is not None and line.startswith('"'):
            parts = line.split('"')
            if len(parts) >= 5:
                current[parts[1]] = parts[3]
    return entities


def convert(path):
    path = pathlib.Path(path)
    data = path.read_bytes()
    lumps = _read_lumps(data)

    planes = [struct.unpack_from("<ffffi", lumps[LUMP_PLANES], i * 20)
              for i in range(len(lumps[LUMP_PLANES]) // 20)]
    vertices = [struct.unpack_from("<fff", lumps[LUMP_VERTICES], i * 12)
                for i in range(len(lumps[LUMP_VERTICES]) // 12)]
    edges = [struct.unpack_from("<HH", lumps[LUMP_EDGES], i * 4)
             for i in range(len(lumps[LUMP_EDGES]) // 4)]
    surfedges = [struct.unpack_from("<i", lumps[LUMP_SURFEDGES], i * 4)[0]
                 for i in range(len(lumps[LUMP_SURFEDGES]) // 4)]
    faces = [struct.unpack_from("<HHiHH4Bi", lumps[LUMP_FACES], i * 20)
             for i in range(len(lumps[LUMP_FACES]) // 20)]

    # model 0 is the world; take only its faces so doors/plats don't clutter the plan
    if len(lumps[LUMP_MODELS]) >= 64:
        model = struct.unpack_from("<9f4i3i", lumps[LUMP_MODELS], 0)
        first_face, num_faces = model[14], model[15]
    else:
        first_face, num_faces = 0, len(faces)

    segments = []
    seen = set()
    bounds = [math.inf, math.inf, -math.inf, -math.inf]

    for face_index in range(first_face, min(first_face + num_faces, len(faces))):
        planenum, _side, firstedge, numedges = faces[face_index][:4]
        if planenum >= len(planes):
            continue
        nx, ny, nz, _dist, _type = planes[planenum]
        if abs(nz) >= WALL_NORMAL_Z:
            continue  # floor or ceiling

        for e in range(firstedge, firstedge + numedges):
            if e >= len(surfedges):
                break
            se = surfedges[e]
            edge = edges[abs(se)]
            v1, v2 = vertices[edge[0]], vertices[edge[1]]

            dx, dy = v2[0] - v1[0], v2[1] - v1[1]
            if math.hypot(dx, dy) < MIN_SEGMENT_2D:
                continue

            zmin = min(v1[2], v2[2])
            zmax = max(v1[2], v2[2])

            # dedupe shared edges / coincident top+bottom projections
            key = (round(min(v1[0], v2[0])), round(min(v1[1], v2[1])),
                   round(max(v1[0], v2[0])), round(max(v1[1], v2[1])), round(zmin / 32.0))
            if key in seen:
                continue
            seen.add(key)

            segments.append([round(v1[0], 1), round(v1[1], 1),
                             round(v2[0], 1), round(v2[1], 1),
                             round(zmin, 1), round(zmax, 1)])
            bounds[0] = min(bounds[0], v1[0], v2[0])
            bounds[1] = min(bounds[1], v1[1], v2[1])
            bounds[2] = max(bounds[2], v1[0], v2[0])
            bounds[3] = max(bounds[3], v1[1], v2[1])

    spawns = []
    for ent in _parse_entities(lumps[LUMP_ENTITIES]):
        if ent.get("classname") in ("info_player_deathmatch", "info_player_start"):
            try:
                x, y, z = (float(v) for v in ent.get("origin", "").split())
                spawns.append([round(x, 1), round(y, 1), round(z, 1)])
            except ValueError:
                pass

    if not segments:
        bounds = [0, 0, 0, 0]
    return {
        "map": path.stem,
        "bounds": [round(b, 1) for b in bounds],
        "segments": segments,
        "spawns": spawns,
    }


# --------------------------------------------------------------------------
def _build_test_bsp():
    """Tiny in-memory v30 bsp: one square room wall face + one spawn."""
    entities = (b'{\n"classname" "worldspawn"\n}\n'
                b'{\n"classname" "info_player_deathmatch"\n"origin" "100 200 36"\n}\n\x00')
    # one vertical wall plane (normal +x) and one floor plane (normal +z)
    planes = struct.pack("<ffffi", 1, 0, 0, 128, 0) + struct.pack("<ffffi", 0, 0, 1, 0, 2)
    vertices = b"".join(struct.pack("<fff", *v) for v in
                        [(128, 0, 0), (128, 256, 0), (128, 256, 128), (128, 0, 128)])
    edges = b"".join(struct.pack("<HH", *e) for e in [(0, 1), (1, 2), (2, 3), (3, 0)])
    surfedges = b"".join(struct.pack("<i", i) for i in range(4))
    # wall face on plane 0 + floor face on plane 1 (must be skipped)
    faces = (struct.pack("<HHiHH4Bi", 0, 0, 0, 4, 0, 0, 0, 0, 0, -1) +
             struct.pack("<HHiHH4Bi", 1, 0, 0, 4, 0, 0, 0, 0, 0, -1))
    model = struct.pack("<9f4i3i", -128, -256, -128, 128, 256, 128, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 2)

    lumps = [b""] * LUMP_COUNT
    lumps[LUMP_ENTITIES] = entities
    lumps[LUMP_PLANES] = planes
    lumps[LUMP_VERTICES] = vertices
    lumps[LUMP_EDGES] = edges
    lumps[LUMP_SURFEDGES] = surfedges
    lumps[LUMP_FACES] = faces
    lumps[LUMP_MODELS] = model

    header_size = 4 + LUMP_COUNT * 8
    body, directory = b"", b""
    offset = header_size
    for lump in lumps:
        directory += struct.pack("<ii", offset, len(lump))
        body += lump
        offset += len(lump)
    return struct.pack("<i", BSP_VERSION) + directory + body


def _selftest():
    import tempfile
    with tempfile.NamedTemporaryFile(suffix=".bsp", delete=False) as f:
        f.write(_build_test_bsp())
        temp_name = f.name
    result = convert(temp_name)
    pathlib.Path(temp_name).unlink()

    assert result["spawns"] == [[100.0, 200.0, 36.0]], result["spawns"]
    # 4 edges of the wall quad: 2 are vertical lines in 2d (skipped),
    # top and bottom project onto the same 2d line (deduped by z bucket? no -
    # they are 128 apart, different z bucket) -> expect exactly 2 segments
    assert len(result["segments"]) == 2, result["segments"]
    for seg in result["segments"]:
        assert {(seg[0], seg[1]), (seg[2], seg[3])} == {(128.0, 0.0), (128.0, 256.0)}, seg
    assert result["bounds"] == [128.0, 0.0, 128.0, 256.0], result["bounds"]
    print("selftest ok:", json.dumps(result))


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    if sys.argv[1] == "--selftest":
        _selftest()
        return
    print(json.dumps(convert(sys.argv[1]), separators=(",", ":")))


if __name__ == "__main__":
    main()
