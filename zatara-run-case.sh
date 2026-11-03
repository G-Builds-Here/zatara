#!/bin/sh
# zatara run-case launcher (XUNIT-V3-MTP): the unix twin of
# zatara-run-case.cmd. It ships at the extension package root (publish_sync
# mirrors it); dirname $0 below is therefore this file's own directory.
#
# Proxy locations, in search order (measured 2026-11-02, owner's
# clean-install cycle): the package-root binary-cache, then Zed's
# extension WORK dir - extensions/work/zatara-netcoredbg/binary-cache
# under the platform data dir - where the extension's own release
# downloader writes. The two are different directories on every real
# install; a launcher that searched only the package root could never
# see a downloaded proxy.
#
# Why a twin exists: Zed gives every contributed task one command string
# for every OS (task templates carry no os field), so no shipped default
# task can name a unix path. On macOS and Linux this file is the payload a
# user-configured task points at; the README's Run-lane section carries the
# one-line recipes.
#
# ZATARA_PROXY_DEV (env): the dev-loop pin - when set it names the proxy
# binary directly and the binary-cache walk is skipped. Shipped sessions
# never carry it.
#
# Single-lane design: the dead-live-stdout defect is a Windows conpty thing
# (a GUI-subsystem process's live stdout never renders in the task
# pseudo-terminal). On unix the proxy is a console process, its stream
# renders through the task terminal directly, so exec streams the row live
# and carries the exit code with no buffering - and no console helper, the
# zatara-run-case asset being win-x64 only.
#
# Pinned self-fetch (owner sighting 2026-11-03, mirrored from the batch
# twin): extension.toml beside this file names the version this install IS.
# The caches are searched for that EXACT asset name first; on a miss the
# launcher downloads it itself - hash-verified against the release's
# manifest.json, renamed into place only after the hash matches - instead
# of making the user Debug one test first. The manifest is downloaded
# fresh on every fetch and never reused (a cached old manifest would list
# no new pinned asset forever). A failed fetch is visible, never silent:
# the newest cached proxy still runs (offline usability) but the output
# names the wanted asset and the WHY, and with nothing cached the decline
# names the failed fetch too. ZATARA_RELEASE_BASE overrides the download
# root (mirror and test seam).
#
# The stale fallback picks the newest zatara-proxy* by mtime, matching the
# batch twin's dir /o-d ordering. Asset file names are plain file names
# (the extension's asset law: no spaces, no separators), so the name stays
# word-safe inside the quoted join below even under msys sh.
set -u
DIR=$(dirname "$0")
CACHE="$DIR/binary-cache"
# Zed's data dir per platform: XDG on Linux, Application Support on macOS.
# Both are probed; whichever exists is searched second.
ZED_WORK_LINUX="${XDG_DATA_HOME:-$HOME/.local/share}/zed/extensions/work/zatara-netcoredbg/binary-cache"
ZED_WORK_MAC="$HOME/Library/Application Support/Zed/extensions/work/zatara-netcoredbg/binary-cache"
BASE="https://github.com/G-Builds-Here/zatara/releases/latest/download"
if [ -n "${ZATARA_RELEASE_BASE:-}" ]; then BASE="$ZATARA_RELEASE_BASE"; fi
WHY=""
# fetch_asset DIR ASSET: download one release asset into DIR, verify its
# sha256 against the freshly-fetched manifest.json, and only then rename
# the .part file to the final name. Sets PROXY on success, WHY on failure;
# every failure path deletes its .part - bytes never reach the final name
# unverified.
fetch_asset() {
  if [ ! -d "$1" ]; then mkdir -p "$1" 2>/dev/null || true; fi
  if ! command -v curl >/dev/null 2>&1; then
    WHY="curl not found"
    return 1
  fi
  if ! curl -fsSL --connect-timeout 10 -o "$1/manifest.json.part" "$BASE/manifest.json"; then
    rm -f "$1/manifest.json.part"
    WHY="manifest download failed"
    return 1
  fi
  mv -f "$1/manifest.json.part" "$1/manifest.json"
  # The manifest is written with sorted keys, so the sha256 line follows
  # the matched asset-name line; take the first such line's quoted value.
  FSHA=$(awk -v pat="\"$2\"" 'index($0, pat) { seen = 1 }
    seen && /"sha256"/ { sub(/.*"sha256" *: *"/, ""); sub(/".*/, ""); print; exit }' "$1/manifest.json")
  if [ -z "$FSHA" ]; then
    WHY="$2 not listed in manifest.json"
    return 1
  fi
  if ! curl -fsSL --connect-timeout 10 -o "$1/$2.part" "$BASE/$2"; then
    rm -f "$1/$2.part"
    WHY="download failed"
    return 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    GOT=$(sha256sum "$1/$2.part" | cut -d' ' -f1)
  else
    GOT=$(shasum -a 256 "$1/$2.part" | cut -d' ' -f1)
  fi
  if [ -z "$GOT" ]; then
    rm -f "$1/$2.part"
    WHY="could not hash the download"
    return 1
  fi
  if [ "$GOT" != "$FSHA" ]; then
    rm -f "$1/$2.part"
    WHY="sha256 mismatch (manifest $FSHA got $GOT)"
    return 1
  fi
  rm -f "$1/$2"
  mv -f "$1/$2.part" "$1/$2"
  PROXY="$1/$2"
  return 0
}
if [ -n "${ZATARA_PROXY_DEV:-}" ]; then
  PROXY="$ZATARA_PROXY_DEV"
else
  VER=""
  if [ -f "$DIR/extension.toml" ]; then
    VER=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$DIR/extension.toml" | head -n 1)
  fi
  case "$(uname -s)" in Darwin) OS=macos ;; *) OS=linux ;; esac
  case "$(uname -m)" in arm64|aarch64) ARCH=arm64 ;; *) ARCH=x64 ;; esac
  if [ -n "$VER" ]; then
    WANT="zatara-proxy-$VER-$OS-$ARCH"
    for DIR_CANDIDATE in "$CACHE" "$ZED_WORK_LINUX" "$ZED_WORK_MAC"; do
      if [ -f "$DIR_CANDIDATE/$WANT" ]; then
        PROXY="$DIR_CANDIDATE/$WANT"
        break
      fi
    done
    if [ -z "${PROXY:-}" ]; then
      fetch_asset "$CACHE" "$WANT"
    fi
  fi
fi
if [ -z "${PROXY:-}" ]; then
  if [ -n "${VER:-}" ]; then
    STALE=1
    for DIR_CANDIDATE in "$CACHE" "$ZED_WORK_LINUX" "$ZED_WORK_MAC"; do
      if [ -n "${PROXY:-}" ]; then break; fi
      if [ -d "$DIR_CANDIDATE" ]; then
        ASSET=$(cd "$DIR_CANDIDATE" && ls -1t zatara-proxy* 2>/dev/null | head -n 1)
        if [ -n "$ASSET" ]; then PROXY="$DIR_CANDIDATE/$ASSET"; fi
      fi
    done
    if [ -n "${PROXY:-}" ]; then
      echo "zatara run-case: pinned $WANT unavailable - $WHY - running the cached proxy instead; Debug one test to refresh, or check the network."
    else
      echo "zatara run-case: could not fetch $WANT - $WHY."
    fi
  fi
fi
if [ -z "${PROXY:-}" ] || [ ! -f "$PROXY" ]; then
  echo "zatara run-case: no zatara-proxy* binary in \"$CACHE\" or Zed's work directory - Debug one test to download the binaries, or set ZATARA_PROXY_DEV to a built proxy for development."
  exit 2
fi
exec "$PROXY" run-case
