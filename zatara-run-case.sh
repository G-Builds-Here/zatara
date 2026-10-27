#!/bin/sh
# zatara run-case launcher (XUNIT-V3-MTP): the unix twin of
# zatara-run-case.cmd. It ships at the extension package root (publish_sync
# mirrors it); dirname $0 below is therefore this file's own directory,
# identical in the work-dir and installed layouts, and the proxy lives
# beside it under binary-cache.
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
if [ -n "${ZATARA_PROXY_DEV:-}" ]; then
  PROXY="$ZATARA_PROXY_DEV"
elif [ -d "$CACHE" ]; then
  ASSET=$(cd "$CACHE" && ls -1t zatara-proxy* 2>/dev/null | head -n 1)
  if [ -n "$ASSET" ]; then
    PROXY="$CACHE/$ASSET"
  fi
fi
if [ -z "${PROXY:-}" ] || [ ! -f "$PROXY" ]; then
  echo "zatara run-case: no zatara-proxy* binary in \"$CACHE\" - reinstall the zatara extension, or set ZATARA_PROXY_DEV to a built proxy for development."
  exit 2
fi
exec "$PROXY" run-case
