#!/usr/bin/env python3
"""Release download-and-verify guard: check served bytes against manifest.json (AC3, LOC-0108).

This is the supply-chain integrity guard the CI download job runs (DR-4/DR-6).
On ubuntu-latest the release workflow downloads the published assets into a
directory plus the release's manifest.json, then runs this over them. Every
asset the manifest lists must be present and its served sha256 must equal the
recorded one -- a corrupt or swapped binary is caught here, before anyone runs
it. The manifest is untrusted input, so listed names must resolve to plain
regular files INSIDE the assets dir (path escapes and symlinks are refused,
never hashed) and every report line is control-character-escaped before it
reaches the log.

On any failure every offending asset is named on its own line and the exit is
non-zero, so the verify job goes red pointing at the specific bad asset:

    CHECKSUM MISMATCH: zatara-proxy-0.1.0-linux-x64  expected <sha>  got <sha>
    MISSING ASSET:     netcoredbg-3.2.0-linux-x64.tar.gz  (listed in manifest, not served)

The deliberately-corrupt-asset fixture that AC3's second half requires is
exercised by scripts/test_verify_release_assets.py, which corrupts one served
asset after building the manifest and asserts this guard exits non-zero naming
it -- so the guard is proven to actually guard, not to pass unconditionally.

Usage:
    python scripts/verify_release_assets.py --manifest dist/manifest.json --assets-dir dist
"""
import argparse
import hashlib
import json
import sys
from pathlib import Path


def sha256_of_file(path: Path) -> str:
    """Stream-hash a served file so a large binary never sits fully in memory."""
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def log_safe(value: object) -> str:
    """Render manifest-controlled text safe for the Actions log. The manifest
    is untrusted input to the verify job: a name (or hash) carrying newlines
    forges extra log lines, and ESC/CR bytes smuggle ANSI/SGR tricks or hide
    content behind a carriage return. Control characters are escaped to
    printable ``\\xNN`` form; printable ASCII passes through untouched."""
    return "".join(ch if 32 <= ord(ch) < 127 else f"\\x{ord(ch):02x}"
                   for ch in str(value))


def is_safe_asset_name(name: str) -> bool:
    """A release asset name must be a plain basename. The manifest is treated as
    untrusted input to the verify job, so a name that climbs out of the release
    directory (a path separator, '..', or an absolute path) is refused rather
    than resolved -- otherwise a crafted manifest could point the guard at a
    file the release never served and call it a match."""
    return (name not in ("", ".", "..")
            and "/" not in name and "\\" not in name
            and Path(name).name == name)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True,
                        help="Path to the release's manifest.json.")
    parser.add_argument("--assets-dir", type=Path, required=True,
                        help="Directory the release assets were downloaded into.")
    args = parser.parse_args()

    try:
        manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        sys.exit(f"VERIFY FAILED: cannot read manifest {args.manifest}: {exc}")
    if not args.assets_dir.is_dir():
        sys.exit(f"VERIFY FAILED: assets dir not found: {args.assets_dir}")

    assets = manifest.get("assets")
    if not assets:
        sys.exit(f"VERIFY FAILED: manifest {args.manifest} lists no assets")

    problems = []
    for entry in assets:
        name = entry.get("name")
        expected = entry.get("sha256")
        if not name or not expected:
            problems.append(f"VERIFY FAILED: manifest entry missing name/sha256: {entry}")
            continue
        if not is_safe_asset_name(name):
            problems.append(
                f"UNSAFE ASSET NAME: {name} (must be a plain file name, "
                f"no path separators or '..')")
            continue
        served = args.assets_dir / name
        # is_file()/open FOLLOW symlinks: a listed name planted as a symlink
        # would have the guard hash a file outside the release dir and score a
        # true-hash match as a pass -- the false-green the name guard exists
        # to prevent. A symlink is refused here, never hashed.
        if served.is_symlink():
            problems.append(
                f"UNSAFE ASSET: {name} (symlink -- release assets must be "
                f"plain regular files)")
            continue
        if not served.is_file():
            problems.append(f"MISSING ASSET: {name} (listed in manifest, not served)")
            continue
        actual = sha256_of_file(served)
        if actual != expected:
            problems.append(
                f"CHECKSUM MISMATCH: {name}  expected {expected}  got {actual}")

    # Two-way check: verification is meaningless if an unlisted file can ride
    # along. An asset served but NOT named in the manifest is exactly as
    # untrusted as a corrupt one -- dropping a manifest line and adding a
    # planted asset must not verify green. The manifest file itself is exempt.
    listed_names = {entry.get("name") for entry in assets}
    served_names = {p.name for p in args.assets_dir.iterdir() if p.is_file()}
    for extra in sorted(served_names - listed_names - {args.manifest.name}):
        problems.append(f"UNLISTED ASSET: {extra} (served but not in manifest)")

    # Sanitized at print time so every manifest- or filesystem-controlled
    # string is covered -- raw newlines/ESC in a name must not forge lines in
    # the Actions log (log injection).
    for problem in problems:
        print(log_safe(problem))
    if problems:
        print(f"VERIFY FAILED: {len(problems)} problem(s); "
              f"manifest lists {len(assets)} asset(s)")
        return 1
    print(f"VERIFY OK: {len(assets)} asset(s) match manifest.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
