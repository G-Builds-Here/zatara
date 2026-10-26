# examples/program-debug

Paste material for debugging a plain .NET program through zatara. The full
user truth (what each path does at launch, what the proxy resolves, when it
refuses, and how a session ends) is the repository README's
[Program debug](../../README.md#program-debug) section; this directory only
carries the files themselves and where each one goes.

| File | Paste into | Path it serves |
|------|------------|----------------|
| `pinned/debug.json` | `.zed/debug.json` | The pinned-program escape hatch: `config.program` names the dll, and discovery is never consulted |
| `pinned/tasks.json` | `.zed/tasks.json` | The build task for the pinned pair: the proxy never builds, so the pinned program needs its own build |
| `solution-rooted/debug.json` | `.zed/debug.json` | The solution-rooted hint-only variant: no `program`, no test target: the proxy resolves the program at launch, solution-first |

Every file is a complete single-element array, so each can be copied to its
destination as-is or have its one element merged into a file you already keep.
Edit the paths inside before use: `src/App` and `C:/work/app` are stand-ins for
your project layout, not settings; every value in a debug.json is yours.

## Where each file goes

From the root of this repository:

```sh
mkdir -p .zed
cp examples/program-debug/pinned/debug.json .zed/debug.json
cp examples/program-debug/pinned/tasks.json .zed/tasks.json
```

```powershell
New-Item -ItemType Directory -Force .zed | Out-Null
Copy-Item examples/program-debug/pinned/debug.json .zed/debug.json
Copy-Item examples/program-debug/pinned/tasks.json .zed/tasks.json
```

The same shape works for `solution-rooted/debug.json` (one file, no build task
of its own; run the shipped **zatara: build (workspace)** task, press F5 for
**zatara: build & debug (workspace)**, and the proxy resolves the program at
launch). If your repo root is not where your
solution lives, edit `projectPath` and `cwd` to the directory holding it: the
solution file there decides the project set.

A pinned entry can also live at user level: one file, read from every repo.
On Windows that is `%APPDATA%\Zed\debug.json`; the README's
[cross-repo pinning recipe](../../README.md#the-cross-repo-pinning-recipe)
states the probe-recorded merge behavior (both levels coexist, project copy
first). The probe recorded its merge observations on Windows; there is no mac
CI: the recipe marks what remains owed.

## What these examples do not show

- The primary path needs no file at all: run the shipped build task and press
  F5. These examples are the escape hatch and the hand-written variant, not a
  required setup.
- The attach lane (`"request": "attach"` + `process_id`) is documented in the
  README and [docs/templates/manual-attach.md](../../docs/templates/manual-attach.md);
  it is not shipped here because a pasted pid is stale the moment you paste it.
