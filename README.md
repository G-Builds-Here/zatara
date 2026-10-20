# zatara-netcoredbg

Run and debug .NET unit tests from the Zed editor gutter, powered by
[netcoredbg](https://github.com/Samsung/netcoredbg).

This README is the user's surface: what you need, how to install, how to use
it, what you can configure, and what to do when a symptom appears. The internal
design, the project's maturity ledger, and the development workflow live in
[CONTRIBUTING.md](CONTRIBUTING.md).

## Requirements

| Need | Detail |
|------|--------|
| Zed | A build whose extension API is `zed_extension_api` 0.7 |
| .NET SDK | `dotnet` reachable on the PATH the adapter inherits |
| netcoredbg | **Windows / Linux / Apple-silicon macOS:** official [Samsung/netcoredbg](https://github.com/Samsung/netcoredbg) releases, fetched automatically and SHA256-verified against the version-pinned release manifest. **Intel macOS:** upstream publishes no Intel macOS build and no Homebrew formula exists — install netcoredbg manually from the releases page (Apple-silicon asset or source build) and keep it on `PATH` (see [Troubleshooting](#troubleshooting)). |
| A test project | xunit, nunit, or mstest. The Debug lane finds the project by walking up from the test file's directory until a folder names exactly one project; the Run lane points `dotnet` at the repository root and lets its own solution-or-unique-project rules answer. An ambiguous folder stays dotnet's honest refusal, never a guess — `ZATARA_FUZZY_MATCH=1` lets the Debug lane take the nearest solution instead. |

## Install

The extension is not on the marketplace yet, so install it as a **dev
extension** — the flow Zed documents for unpublished extensions. `crates/zatara`
is the extension directory: it holds `extension.toml`, the wasm crate, the
`languages/` assets, and `debug_adapter_schemas/`, all of which Zed loads
relative to that directory.

1. Have Rust installed via rustup. Zed compiles the extension itself on install
   and adds the `wasm32-wasip2` target automatically when rustup owns the
   toolchain.

2. In Zed, open the command palette and run `zed: install dev extension`, then
   select **`crates/zatara`** (not the repo root — the root `Cargo.toml` is a
   workspace, and Zed builds the extension from its own directory). Zed builds
   the wasm and loads it; `Zed.log` (`zed: open log`) shows the build output if
   anything fails.

3. Route C#'s gutter debug action to the adapter — the one-time settings entry
   in [Settings](#settings). No environment variable is needed: the in-proxy
   runner is on by default (`ZATARA_RUNNER=0` opts back to the plain-relay
   dev seam).

4. Restart Zed after changing the extension crate — dev extensions are built at
   install time, not hot-reloaded. The proxy and netcoredbg binaries are
   resolved per session ([binary resolution order](#adapter-environment)) — you
   do not install them by hand unless you want to.

## First gutter debug

Use a small project so the whole flow is visible. This repo's own
`fixtures/dotnet/Basic` fixture (`Basic.AddTests.AddsOneAndTwo`) is the target
the suites exercise.

1. Open the test file. Runnables shipped in `crates/zatara/languages/csharp/runnables.scm`
   put a gutter `>` on test classes and methods.
2. Click the `>` on a test method. **Run** executes the plain filtered
   `dotnet test` that `crates/zatara/languages/csharp/tasks.json` ships — no debugger
   involved. **Debug** starts a zatara session.
3. Watch the build stream. The proxy runs `dotnet build` for the project first
   and its output arrives in the session as it happens.
4. The test step starts paused. The proxy reads the paused host's pid from the
   vstest banner (`Process Id: 34060, Name: testhost`) and attaches netcoredbg
   to it. Nothing is written to a pid file or to shared temp — the pid travels
   over the session's own stream.
5. Set a breakpoint in the test body, then let the run continue to it. Bind,
   hit, step, inspect.
6. Press **Continue** to finish the test. The vstest summary (`Passed!`) arrives
   in the Console and the session closes itself.
7. Stepping off the last line of a test lands inside xunit's `TestInvoker` —
   that is the framework calling your test, not a bug in this extension. Press
   Continue.

The debug path answers only an exact-FQN filter (`ZATARA_TEST_FILTER` set to
`FullyQualifiedName=` plus one full name), and it declines a substring filter
(`FullyQualifiedName~`) on purpose — that is how a debug session silently ends up
attached to the wrong set of tests. The no-debugger **Run** lane is a different
matter and legitimately selects a set: `crates/zatara/languages/csharp/tasks.json` ships substring
filters for its "Run class" and "Run case" actions, and exact FQNs for a single
method.

## What you see

The Console tab is where the session narrates itself. The **order** below is the
sequence CI asserts for a real session; the individual lines are the shapes the
tools print, so treat pids and counts as illustrative:

```text
Console
  build:  dotnet build fixtures/dotnet/Basic/Basic.csproj        (MSBuild output, live)
  test:   dotnet test --filter FullyQualifiedName=Basic.AddTests.AddsOneAndTwo
  test:   Process Id: 34060, Name: testhost                       (paused testhost)
  ...     attach accepted, breakpoints bound                      (stopped at breakpoint)
  test:   Passed!  - Failed: 0, Passed: 1, Skipped: 0             (vstest summary)
  session exited, terminated                                      (panel closes)
```

The gutter and panel layout, schematically:

```text
        gutter                                                          ┌─────────────────┐
  ┌──┐ ┌──────────────────────────────────────────────┐                 │ Console         │
  | >| | public class AddTests                        │   ┌───────────┐ │  build: …       │
  |  | | {                                            │   │ Debug     │ │  test: Process  │
  | >| |     [Fact] public void AddsOneAndTwo() {  ←──┼──▶│ panel:    │ │        Id: 34060│
  └──┘ |         Assert.Equal(3, Calc.Add(1, 2));     │   │ stacks,   │ │  test: Passed!  │
        |     }                                        │   │ locals,   │ │                 │
        └──────────────────────────────────────────────┘   │ breakpoints│ └─────────────────┘
                                                            └───────────┘
```

A recorded GIF of a live editor session belongs here once the editor wiring in
[Status](CONTRIBUTING.md#status) lands. Until then, both halves of the picture above are
derived from checks you can run, not from a description:

- **Which gutter icons appear, and the exact filter each click emits**, are
  asserted by the language-asset harness — `npm test` in the repository's
  `tests/langassets`, which drives the shipped `runnables.scm` and `tasks.json`
  through the pinned C# grammar (no network, no Zed instance).
  It is a local suite: CI does not run it.
- **The Console sequence** is what the CI gutter-flow job asserts about a real
  session — build, pause banner, attach, `Passed!`, teardown.

So the schematic will not quietly drift from behavior: change the query or the
session, and the check that produced this picture goes red first.

## Settings

### Zed settings

Route C#'s gutter debug action to this adapter. Zed uses the **first** entry of
the per-language `debuggers` list, and it matches the language key exactly
against the language's `name` as defined by the C# extension's own
`config.toml` — which is **`CSharp`**, not the prettified `C#` you see in the
language selector. An unrecognized key is ignored silently, so a plausible
misspelling produces no error and no debugger:

```json
{
  "languages": {
    "CSharp": {
      "debuggers": ["zatara-netcoredbg"]
    }
  }
}
```

To pin the adapter binary instead of letting it be resolved, use Zed's
per-adapter override, keyed by the adapter name (`dap.<adapter>.binary`). What
it names for this extension is the **proxy** — that is the binary Zed spawns:

```json
{
  "dap": {
    "zatara-netcoredbg": {
      "binary": "C:/tools/zatara/zatara-proxy.exe"
    }
  }
}
```

There is no **Zed setting** for the netcoredbg path: on Windows, Linux, and
Apple-silicon macOS that binary comes from the verified cache or the pinned
release download, and on Intel macOS from your PATH or a Homebrew-prefix
install (upstream ships no Intel build) — the chains are in
[binary resolution](#adapter-environment) below. (`dap.<adapter>.args` exists in Zed — leave it
alone for this adapter. Zed documents it as overriding the arguments it would
otherwise pass, and this adapter's arguments carry the resolved netcoredbg path,
so an override there produces a session with no debugger.)

### Adapter environment

Zed spawns the adapter with your shell environment plus the keys the extension
hands it, so the flags below are armed by exporting them in the environment Zed
starts from (a shell profile, or launching Zed from a terminal).

| Name | Default | Effect |
|------|---------|--------|
| `ZATARA_RUNNER` | **on** | The in-proxy runner (build, paused test, attach) is ON by default — it is the only mode with unit-test support. Set `0` to opt out into the plain-relay dev seam (launch passed straight to netcoredbg). Any other value, including unset, stays on. |
| `ZATARA_RUNNER_BANNER_TIMEOUT_SECS` | 30 | How long the runner waits for the vstest pause banner before giving up. Raise it on a machine where the first test host is slow to start. |
| `ZATARA_FUZZY_MATCH` | off | Debug lane only. When the project search cannot decide — a folder naming several projects with no name match, or no ancestor folder naming one project at all — the nearest `.sln` going up answers instead: `dotnet test` builds the solution and your filter selects the tests. Set `1` to arm; two solutions in one folder stay dotnet's ambiguity error, and a provable answer always beats the fallback. The **Run lane** is a static task and cannot consult this: Zed has no per-OS task command, so Run points `dotnet test` at the repository root and trusts dotnet's own root resolution. |
| `ZATARA_LAUNCH_MODE` | unset | Forces the dispatch lane a launch takes: `project` or `test`, exact values only — anything else (wrong case included) acts as unset and the config's own shape answers instead. Read by the proxy at its env boundary; never a wire field. The [Program debug](#program-debug) section is where it earns attention. |
| `ZATARA_PROGRAM_BUILD` | shipped with the door task — `1` | The exact tag pair the shipped **zatara: build & debug (workspace)** task carries — the claimable entry the locator claims as a program door and the debug scenario derives from (the `ZATARA_TEST_FILTER` law again: near-miss values and other names claim nothing). The plain **zatara: build (workspace)** entry carries no tag and claims nothing. Not yours to set. |
| `ZATARA_WORK_DIR` | set by the extension | The extension's work directory, which is the only place the proxy may write diagnostics. The extension always hands this over; you never set it. |
| `ZATARA_RELEASE_WARNING` | reserved — set by nothing today | The seam a degraded session's reason *could* travel on. The extension logs its note to Zed's log directly and keeps this unset (owner ruling 2026-10-13: the degraded fact is a log note, never editor UI); a future proxy that consumes it must log it too. You never set it. |

Two binaries are resolved per session, by different parties, so the two chains
are worth keeping apart.

**The proxy** — the program Zed actually spawns:

1. your `dap.zatara-netcoredbg.binary` override — Zed's own precedence, used
   verbatim with no network;
2. otherwise the extension answers: its work-dir cache, re-hashed against the
   pinned release manifest (a cached binary is never trusted from memory), and
   failing that the pinned release download, verified before it is executed.

   When the release cannot be **reached** at all — offline, or release assets
   a client cannot download anonymously (a private repository answers 404) —
   the session still runs: the settings override or the cached binary is used,
   and a note naming what went unverified goes to **Zed's log** (guest and
   adapter stderr), not the editor. An unreachable release is a quiet log
   fact, never a blocker; a release that is reached and **fails** verification
   (a checksum mismatch) still refuses the session outright. Reachability
   degrades; integrity does not. Offline workflows work: the first successful
   session caches the binaries, and every later session — online or not —
   runs from them.

**netcoredbg** — which the extension resolves the same two-step way and then
names for the proxy:

1. the resolved path, carried in the proxy's arguments as
   `--netcoredbg <path>`;
2. `ZATARA_NETCOREDBG`, an environment pin that ranks below the argument and
   above a PATH lookup. This is the seam the proxy's own suites and CI inject a
   stand-in debugger through, and nothing in a normal session ever sets it — it
   is not a supported setting, and if you are not driving those suites you do
   not want it;
3. nothing named it: plain `netcoredbg` found on your PATH. On Intel macOS
   the extension deliberately names no path at all (upstream ships no Intel
   build), which sends the proxy to probe settings, then PATH, then Homebrew
   prefixes — a fine place a manual install can live
   ([Troubleshooting](#troubleshooting)). Apple-silicon macOS names the
   official osx-arm64 asset like every other served platform.

### Debug scenario keys

A hand-written debug.json for this adapter is one launch config, and the key set
is deliberately narrow: it mirrors, key for key, the same contract that crosses the
extension/proxy boundary (schema keys equal serde fields — SCHEMA_EQ). One key set
serves both lanes: a config carrying a program or request key is a
program/attach config, and every other config is a test-lane config that must name
its target. Keys beyond these are **rejected, not ignored**, which is what makes a
typo surface as an error instead of a wrong session.

| Key | Level | Meaning |
|-----|-------|---------|
| `label` | scenario | Name shown in Zed's debug UI. |
| `adapter` | scenario | Must name this adapter. |
| `config` | scenario | The launch config object. |
| `projectPath` | config | Absolute path to the `.csproj` the test lives in. |
| `fullyQualifiedName` | config | One exact test FQN. A parameterized case's display name is valid; a substring filter is not. |
| `cwd` | config | The working directory the proxy builds and tests in. |
| `env` | config | Environment the proxy forwards to build and test, as a map of string to string. |
| `resultsLog` | config | Optional, default off. Set to true and the proxy appends dotnet's own `--logger trx` to the test step, so each run also writes a .trx results file under TestResults/ in the working directory. |
| `program` | config | Program mode: the dll or apphost to debug. Its presence selects the program branch and releases the config from the test-lane keys. |
| `args` | config | Arguments handed to the program. |
| `request` | config | Which lane the config asks for: a program launch or a manual-pid attach. |
| `process_id` | config | Manual-pid attach target, snake_case on purpose — the one deliberate exception to the camelCase vocabulary. |
| `scope` | config | Gutter-click scope: method (default when absent), case, class, or file. The shipped gutter locator never emits it. |
| `framework` | config | Test framework behind the target (nunit, mstest, xunit), consulted only for case scope. |
| `caseArguments` | config | Displayed arguments of the invoked case, as displayed tokens. |
| `methodParameters` | config | The invoked method's parameter names, zipped with `caseArguments` for the named-argument display. |

The shipped schema is the machine-readable form of this table:
`crates/zatara/debug_adapter_schemas/zatara-netcoredbg.schema.json`. The locator emits exactly
the test-lane fields it needs — `projectPath`, `fullyQualifiedName`, `cwd`, `env` —
and nothing else: no scope or framework at all (both are fixed absent and
serialize as omitted keys), and it never flips `resultsLog`: a gutter Debug session
runs unlogged, and only a hand-written config opts into the results file. A gutter
Debug session therefore exists only for one exact test; class and parameterized-case
clicks run on the task lane instead (see [Templates](#templates)).

### Diagnostics gates

Both are default-off: an unset flag is an off flag, and only the value `1` turns
one on — an unrecognized value stays off too. Each writes exactly one file, and
only inside the extension work dir; never shared temp, never next to your
sources. A gate armed while no work dir is known stays off rather than inventing
a location.

| Flag | Default | Turns on | File |
|------|---------|----------|------|
| `ZATARA_ENGINE_LOGGING` | off | netcoredbg's own wire-traffic log (`--engineLogging`), i.e. every DAP message the adapter and debugger exchange | `netcoredbg-engine.log` |
| `ZATARA_PROXY_LOG` | off | the proxy's session lifecycle log: spawns, the parsed pause, attach, teardown | `zatara-proxy.log` |

## Templates

Copy-in starting points live in [`docs/templates/`](docs/templates/):

- [`gutter-test.debug.json`](docs/templates/gutter-test.debug.json) — one exact
  test, the shape a gutter click produces.
- [`class-scope.md`](docs/templates/class-scope.md) — a whole class runs as a
  gutter **Run**: `tasks.json` hands `dotnet test` a substring filter directly,
  and the shipped debug flow answers only exact test names, so there is no
  class-scope file to copy in.
- [`manual-attach.md`](docs/templates/manual-attach.md) — attach to a test host
  you started yourself, without this extension driving the session.

Every shipped debug.json template is validated against the shipped schema by the
test suite, so a template cannot rot into a schema warning; the `.md` pages (class
scope, manual attach) explain why those flows are not copy-in configs.

## Program debug

The gutter lanes debug tests. This section is the user truth for the other
shape — a plain .NET program — as the shipped binaries and the probe notes
record it. One law: every sentence here describes behavior that ships, and
every win-only claim carries its recording status with it.

### The primary path: shipped task, then F5

Zero `.zed/` files. With the extension installed and the
`languages.CSharp.debuggers` mapping from [Install](#install) in place:

1. Run the shipped **zatara: build (workspace)** task (command palette,
   `task: spawn`, pick it). It runs `dotnet build` at the worktree root and
   claims nothing — the untagged plain build entry is safe to spawn at any
   point in this flow, and the program step 2 launches is the output this
   step just built.
2. With a C# file open, press F5 and pick **zatara: build & debug (workspace)**.
   This is the debug flow. The scenario is derived from the shipped task entry
   of the same name, which carries the exact tag pair `ZATARA_PROGRAM_BUILD=1`
   — the one thing that makes a task claimable as the program door, never
   something you set by hand. The visibility probe (pinned at Zed stable
   1.24.2) recorded the template scenario listed once an active C# buffer plus
   that `debuggers` mapping are in place — both are conditions the install
   already asks of you. A pinned entry (below) is listed unconditionally by
   comparison.

   One task-list caveat, measured live at Zed 1.24.2: the entry named
   **zatara: build & debug (workspace)** also appears in the task list — it is
   the surface the scenario derives from, not the entry to run. It is now
   self-explaining: spawned from the task list it builds nothing and prints
   the instruction instead (the scenario owns the build — the proxy builds
   inside its pty). The spawn still moves the entry out of the fresh task list
   for the window's life, and the scenario derives from that fresh entry, so
   the F5 entry disappears until Zed is restarted — recoverable, and the
   printed line says so. Build with the plain entry (step 1); debug with
   F5 (step 2).
   Two picker footnotes from the same measurement: extension task templates
   are hidden from both pickers, by Zed's default, whenever the language
   server ships its own tasks — set `"tasks": { "prefer_lsp": false }` in Zed
   settings when zatara entries never list at all; and a spawned task or
   scenario can always be relaunched from the pickers' history section,
   which the fresh-list filter does not touch.
3. At launch the proxy resolves the program, solution-first: a solution at the
   root decides the project set, otherwise exactly one exe project under the
   root wins, and its built dll — the one next to its csproj — is what runs.
   The resolution never guesses: an ambiguous set fails naming every candidate
   project, a root with nothing runnable fails naming the scanned root, and a
   program the disk does not have fails naming the exact path — before the
   debugger starts, never after.

Breakpoints and stepping work as in a test session, and the debuggee's stdout
comes back raw into the Console. A program session ends the way a session
should: a nonzero exit arrives first as an error-level output line carrying the
exit information, then `exited` and `terminated` close the stream — a nonzero
exit maps to a failed session, and a clean exit closes without an error
announcement. Stopping the session takes the adopted tree (program and
netcoredbg) down with it, zero orphans, and a second session in the same
project is untouched by the first one's teardown.

`ZATARA_LAUNCH_MODE` is the knob for when the inference is not what you want:
the exact values `project` or `test` force the dispatch lane, nothing else acts
(a typo or wrong case never reroutes a launch — the config's own shape answers
instead), and it never travels the wire as a field. The program flow's truth is
recorded on Windows and Linux CI lanes; mac has no CI — the caveat travels with
the win-only claims below.

### The escape hatch: pin the program

A config carrying `program` is a program launch: discovery is never consulted,
the path you wrote runs, and `args` and `cwd` ride alongside it. That is the
pinned path — `.zed/` per repo, or user level per the
[cross-repo pinning recipe](#the-cross-repo-pinning-recipe). The worked pair
ships in [`examples/program-debug/`](examples/program-debug/), and the
directory's README names where each file goes. The pinned program is yours to
build — the proxy never builds — so a pinned pair names its build:

```sh
dotnet build src/App/App.csproj
```

### Attach to a running pid (Should-rated, probe-gated)

Manual-pid attach ships: a config carrying `"request": "attach"` and
`process_id` attaches netcoredbg to a running process through the same relay.
Zed routes the shape as an attach, and the proxy vouches for the pid with the
OS first — a pid naming no running process answers a clean DAP error naming the
pid. The status is truth, not polish: this is a Should-rated lane, and its
behavior is probe-gated — on the pinned netcoredbg build (3.2.0-1092) the
LOC-0134 spike measured that breakpoints do **not** bind in attach sessions.
Interrupt, inspect, and resume work; do not plan a breakpoint-driven attach
session on this build. A session stop kills the attached-to process either
way: `terminateDebuggee: true` kills it, and so does the DAP `terminate`
that Zed's stop button sends (live-verified 2026-10-20). The
attach truth is recorded on Windows — no mac CI. The rendezvous steps are in
[manual-attach.md](docs/templates/manual-attach.md).

### The cross-repo pinning recipe

Pinning a profile at user level means one file, read from every repo. What
follows is the recorded observation from the LOC-0135 probe — quoted from
[docs/LOC-0135-user-level-merge-probe.md](docs/LOC-0135-user-level-merge-probe.md),
audited on Windows at Zed 1.24.2 (no mac CI) — and where the probe marked a
step human-walkthrough-owed, the step appears as a marked gap below, never as
an invented UI instruction.

- **Cross-repo visibility (VERDICT-1).** Entries in the user-level
  `debug.json` / `tasks.json` are visible from every repo: each project opens
  its own settings observer on the single user files, and the global map is
  appended to every scenario and task list.
- **Collision (VERDICT-2).** Neither level replaces the other — same-label
  entries from user and project level COEXIST and both are listed. The project
  copy sorts first, and a label lookup resolves to the project copy; debug
  scenarios carry no dedup at all. A user-level entry is therefore safe to pin
  across repos, but a project entry with the same label shadows it
  positionally — to replace behavior wholesale, the labels must differ.
- **Where the user-level files live on Windows (VERDICT-3).**
  `%APPDATA%\Zed\debug.json` and `%APPDATA%\Zed\tasks.json`. The tasks docs
  say the global `tasks.json` is "usually located in
  `~/.config/zed/tasks.json`" — on Windows that path is never read; the source
  resolves AppData, and the recipe follows the source (both cited in the probe
  note).
- **Project-level locations (VERDICT-4).** Per worktree: `.zed/tasks.json`,
  `.vscode/tasks.json`, `.zed/debug.json`, `.vscode/launch.json`. A repo-root
  `debug.json` or `tasks.json` is not read. If a `.zed` dir contributes
  anything, that worktree's `.vscode` entries are suppressed, and project
  config files wait until the worktree is trusted.
- **The modal gate (VERDICT-5).** A scenario is listed only if its DAP adapter
  is installed — a user-level zatara profile shows up only in windows where the
  zatara extension is installed.
- **"Pinned entry" (VERDICT-6).** 1.24.2's debugger UI has no pin feature; a
  pinned entry is an entry pinned in the user-level config file — and per
  VERDICT-1 those genuinely live across repos. The recency list is per-project
  and in memory, not cross-repo.

One user-level pin for the pinned-program path — the same array shape, one more
element, in `%APPDATA%\Zed\debug.json` on Windows:

```json
{
  "label": "zatara: pinned App (user level, every repo)",
  "adapter": "zatara-netcoredbg",
  "config": {
    "program": "C:/work/app/src/App/bin/Debug/net9.0/App.dll",
    "cwd": "C:/work/app/src/App"
  }
}
```

The live-UI half of this recipe was marked owed by the probe, then walked on
live Zed 1.24.2 (Windows) with extension v0.1.3 on 2026-10-20 — the probe's
own steps, not invented click paths:

- confirmed live 2026-10-20: the picker lists the user profile and BOTH copies
  of the collision twin, with the project copy rendered above the user copy —
  the picker groups by source file: project block first, then user block
- confirmed live 2026-10-20: a second window on another repo shows the user
  profiles plus its own project profiles only — project entries do not cross
  repos, user entries do
- confirmed live 2026-10-20: the user global task lists in both repos while
  each repo's own task lists only at home
- measured 2026-10-20: the debug picker does NOT hot-reload a deleted or
  edited `%APPDATA%\Zed\debug.json` — a running Zed keeps the old list, so
  restart it after editing user-level files

The recipe's win-only parts — the AppData path and the probed picker — carry
the same limitation stated everywhere in this section: recorded on Windows, no
mac CI.

## Troubleshooting

| Symptom | Cause and fix |
|---------|---------------|
| Zed never offers `zatara-netcoredbg` as a debugger, or the gutter Debug action goes nowhere | The adapter/locator declaration gap in [Status](CONTRIBUTING.md#status), or the per-language `debuggers` key does not match your language name exactly. |
| Gutter Run/Debug icons do not appear right after installing the extension | Zed attaches an extension's tasks and test markers when a project opens — after installing, close the project and reopen it (or restart Zed) before the icons show. Fresh-reader observation, 2026-10-14: the gutter appeared only after the project was removed and reloaded. |
| "None of the locators for task … completed successfully" | The task is not a zatara gutter action: it needs `ZATARA_TEST_FILTER` with an exact `FullyQualifiedName=` filter. (Where the test sits — the project folder or any folder inside it — the proxy's project search resolves the Debug lane's target itself.) |
| Run prints MSB1003 or MSB1011 | The Run lane runs `dotnet test` against the repository root and lets dotnet find the project there (a solution, or exactly one project, subfolders included). A root that holds several projects with no solution is dotnet's ambiguity error to refuse — arm `ZATARA_FUZZY_MATCH=1` and the Debug lane will take the nearest solution instead, or add a solution at the root. |
| The session ends immediately with no test output | `ZATARA_RUNNER=0` is set (the runner is on by default since 2026-10-14) — remove it. |
| The debug panel never says passed or failed | The run's report card is the **Console** tab: the vstest summary (`Passed! - Failed: 0, ...`) lands there when the session closes. Zed's debug panel carries no test-run state — that is an upstream gap (see [Upstream backlog](#upstream-backlog)), not something an extension can add. |
| Session runs but the release is unreachable (private repository — GitHub answers 404 to anonymous asset downloads — or offline) | Working as designed: the session runs from the settings override or the cached binaries, and Zed's log carries a `degraded session` note naming what went unverified. Nothing appears in the editor. If there is no proxy on disk at all the session cannot start — point `dap.zatara-netcoredbg.binary` at one, run one session while a release is reachable so the cache fills, or publish the releases anonymously. |
| "test host did not pause within 30s" | The framework never parked a `testhost`. Confirm the target is a runnable test, or raise `ZATARA_RUNNER_BANNER_TIMEOUT_SECS`. |
| macOS: netcoredbg not found | **Apple-silicon:** zatara downloads the official osx-arm64 build automatically — this message means the pinned release was unreachable and nothing was cached yet; install netcoredbg from [Samsung/netcoredbg releases](https://github.com/Samsung/netcoredbg/releases) and keep it on `PATH`. **Intel:** upstream publishes no Intel macOS build and there is no Homebrew formula — build netcoredbg from source and put it on `PATH`. |
| The next build fails with file locks after a stopped session | A test host survived the stop. Retry the session; the proxy records every child it adopts and takes the whole tree down on stop and on crash, so a survivor is a bug worth reporting. |

## Upstream backlog

Four known gaps belong to upstream projects rather than to this extension: the
test-run completion affordance in Zed's debug panel, coordination with the C#
language assets, locator ergonomics when no build task is involved, and
per-OS commands in extension-contributed tasks (the shape the Run lane needs
and Zed's task schema does not offer). They are
listed with a one-line status each in
[`UPSTREAM-BACKLOG.md`](UPSTREAM-BACKLOG.md), which lives in this repository —
nothing is filed against a third-party tracker until the owner opens that gate on
the project's maturity.

## Contributing

Build commands, the internal design, the maturity ledger, and the full
postmortem of the 2026-10 build are in [CONTRIBUTING.md](CONTRIBUTING.md).

## Credits

- [Samsung/netcoredbg](https://github.com/Samsung/netcoredbg) — the debug
  adapter this extension drives; its releases are fetched SHA256-verified.
- [zed-industries/zed](https://github.com/zed-industries/zed) — the editor, the
  extension API, and the task/locator model the gutter actions ride on.
- The .NET test platform's `VSTEST_HOST_DEBUG` pause contract — the parked
  `testhost` is the seam that makes debugging a test possible at all.
- xunit, nunit, and mstest — the frameworks whose discovery and filter syntax
  the gutter actions speak.
- The proven Windows-only prototype this design follows — a reference, not a
  fork.

## License

MIT
