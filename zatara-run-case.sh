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
# The cache walk picks the newest zatara-proxy* by mtime, matching the
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
if [ -n "${ZATARA_PROXY_DEV:-}" ]; then
  PROXY="$ZATARA_PROXY_DEV"
else
  for DIR_CANDIDATE in "$CACHE" "$ZED_WORK_LINUX" "$ZED_WORK_MAC"; do
    if [ -d "$DIR_CANDIDATE" ]; then
      ASSET=$(cd "$DIR_CANDIDATE" && ls -1t zatara-proxy* 2>/dev/null | head -n 1)
      if [ -n "$ASSET" ]; then
        PROXY="$DIR_CANDIDATE/$ASSET"
        break
      fi
    fi
  done
fi
if [ -z "${PROXY:-}" ] || [ ! -f "$PROXY" ]; then
  echo "zatara run-case: no zatara-proxy* binary in \"$CACHE\" or Zed's work directory - Debug one test to download the binaries, or set ZATARA_PROXY_DEV to a built proxy for development."
  exit 2
fi
exec "$PROXY" run-case
