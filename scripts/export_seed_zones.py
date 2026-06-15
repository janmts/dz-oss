#!/usr/bin/env python3
"""Regenerate the bundled first-run drift zones from the local authoring DB.

The app seeds `src-tauri/src/seed_zones.json` into a fresh `drift_zones` table on
first launch (see `db::seed_drift_zones_if_empty`). That file is a generated
artifact: it is the set of zones authored in the developer's local `sessions.db`,
exported into the `DriftZoneInput` shape (camelCase; no id / timestamps / derived
end gates — those are assigned on insert).

Run this whenever the authored zone set changes (a new zone, an edited boundary,
a renamed zone, a scoring-config tweak) and commit the regenerated JSON.

    python scripts/export_seed_zones.py            # reads %LOCALAPPDATA%/fh6-tel/sessions.db
    python scripts/export_seed_zones.py --db PATH  # explicit source DB

The DB is opened read-only.
"""

import argparse
import json
import os
import sqlite3
import sys

# Working-title cleanups applied on export. Keyed by a stable substring of the
# *source* name (not the volatile autoincrement id) so the rename still fires if
# the zone is ever re-created with a new id or exported from another DB. First
# matching entry wins.
RENAME_BY_SUBSTRING = [
    # source "Kawazu Nanadaru Loop Bridge ELEVATION FIX TEST" -> clean public name
    ("Kawazu Nanadaru Loop Bridge", "Kawazu Nanadaru Loop Bridge"),
]

# Authoring-placeholder tokens that must never reach the public seed. After the
# rename pass, any surviving name containing one of these (case-insensitive)
# aborts the export loudly rather than silently shipping a working title.
PLACEHOLDER_TOKENS = ["TEST", "ELEVATION FIX", "PLACEHOLDER", "TODO", "WIP", "DEBUG", "XXX"]


def clean_name(raw: str) -> str:
    for needle, clean in RENAME_BY_SUBSTRING:
        if needle in raw:
            return clean
    return raw


def xz_only(points):
    """Boundary/gate points are 2D (ZonePoint is x/z only). Drop any stray
    per-anchor `y` — the rejected per-anchor height field — so the seed carries
    only what the Rust model reads; height lives in scoringConfig.levels."""
    return [{"x": p["x"], "z": p["z"]} for p in points]


def default_db() -> str:
    base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    return os.path.join(base, "fh6-tel", "sessions.db")


def main() -> int:
    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    default_out = os.path.join(repo_root, "src-tauri", "src", "seed_zones.json")

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=default_db(), help="source sessions.db")
    ap.add_argument("--out", default=default_out, help="seed JSON destination")
    args = ap.parse_args()

    if not os.path.exists(args.db):
        print(f"source DB not found: {args.db}", file=sys.stderr)
        return 1

    con = sqlite3.connect(f"file:{args.db}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    rows = con.execute(
        """SELECT id, name, description, active,
                  left_boundary_json, right_boundary_json,
                  split_gates_json, scoring_config_json
           FROM drift_zones ORDER BY id ASC"""
    ).fetchall()
    con.close()

    zones = []
    for r in rows:
        zones.append(
            {
                "name": clean_name(r["name"]),
                "description": r["description"],
                "active": bool(r["active"]),
                "leftBoundary": xz_only(json.loads(r["left_boundary_json"])),
                "rightBoundary": xz_only(json.loads(r["right_boundary_json"])),
                "splitGates": [xz_only(g) for g in json.loads(r["split_gates_json"])],
                "scoringConfig": json.loads(r["scoring_config_json"]),
            }
        )

    # Refuse to ship an authoring placeholder name (a missed/failed rename).
    for z in zones:
        upper = z["name"].upper()
        leaked = [t for t in PLACEHOLDER_TOKENS if t in upper]
        if leaked:
            print(
                f"refusing to export: zone name {z['name']!r} still contains "
                f"placeholder token(s) {leaked}; add a RENAME_BY_SUBSTRING entry.",
                file=sys.stderr,
            )
            return 1

    # One compact zone object per line inside a pretty top-level array, so a git
    # diff is zone-granular instead of one multi-kilobyte line.
    body = ",\n".join(json.dumps(z, separators=(",", ":")) for z in zones)
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("[\n" + body + "\n]\n")

    print(f"wrote {len(zones)} zones -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
