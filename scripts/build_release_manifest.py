"""Release manifest builder: hash every staged asset into manifest.json (AC2, LOC-0108).

The release workflow stages one built artifact per target platform (the
zatara-proxy binary and the pinned netcoredbg asset for each of win-x64,
linux-x64, linux-arm64, macos-x64, macos-arm64) into a directory, then runs
this to produce the version-pinned SHA256 manifest that ships alongside them.

Each manifest entry records the asset name, the pinned release version, and the
sha256 of the bytes AS STAGED -- the same bytes GitHub will serve -- so the
download-and-verify job (scripts/verify_release_assets.py) has something
authoritative to check served bytes against.

Determinism (AC2 scenario 'Manifest is deterministic for a pin set'): output is
sorted by asset name with sorted JSON keys and carries NO timestamp or run id,
so two runs over the same asset set + version are byte-identical. A re-run for
the same tag with no code change therefore reproduces the manifest exactly.

Usage:
    python scripts/build_release_manifest.py --assets-dir dist --version 0.1.0
    python scripts/build_release_manifest.py --assets-dir dist --version 0.1.0 --output dist/manifest.json
"""
import argparse
import hashlib
import json
import sys
from pathlib import Path
def sha256_of_file(path: Path) -> str:
    """Stream-hash a file so a large binary never has to sit fully in memory."""
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()
def build_manifest(assets_dir: Path, version: str, manifest_name: str) -> dict:
    """Return the manifest dict for every regular file in assets_dir, excluding
    the manifest file itself. Entries sorted by name for a deterministic order."""
    entries = []
    for path in sorted(assets_dir.iterdir()):
        if not path.is_file() or path.name == manifest_name:
            continue
        entries.append({
            "name": path.name,
            "version": version,
            "sha256": sha256_of_file(path),
        })
    entries.sort(key=lambda e: e["name"])
    return {"assets": entries}
def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets-dir", type=Path, required=True,
                        help="Directory holding the staged release assets.")
    parser.add_argument("--version", required=True,
                        help="Pinned release version stamped on every asset entry.")
    parser.add_argument("--output", type=Path, default=None,
                        help="Manifest path (default: <assets-dir>/manifest.json).")
    args = parser.parse_args()
    if not args.assets_dir.is_dir():
        sys.exit(f"MANIFEST BUILD FAILED: assets dir not found: {args.assets_dir}")
    if not args.version.strip():
        sys.exit("MANIFEST BUILD FAILED: --version must be a non-empty string")
    out_path = args.output or (args.assets_dir / "manifest.json")
    manifest = build_manifest(args.assets_dir, args.version, out_path.name)
    if not manifest["assets"]:
        sys.exit(f"MANIFEST BUILD FAILED: no assets staged in {args.assets_dir} "
                 f"(a release with zero assets is not publishable)")
    serialized = json.dumps(manifest, indent=2, sort_keys=True,
                            ensure_ascii=True) + "\n"
    out_path.write_text(serialized, encoding="utf-8")
    print(f"manifest OK: {out_path} lists {len(manifest['assets'])} asset(s) "
          f"at version {args.version}")
    return 0
if __name__ == "__main__":
    sys.exit(main())
