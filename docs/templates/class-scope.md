# Class scope: how a class really runs, and what a debug.json cannot

Zed's gutter puts a `>` on a test class as well as on each method. Clicking the
class `>` and choosing **Run** runs every test in the class. This page is not a
config to copy in — it is the reason there is no class-scope config to copy in,
and what to reach for instead.

## A hand-written debug.json is not the class path

The shipped scenario schema mirrors the launch-config contract key for key, with
`additionalProperties: false`: the test-lane keys (`projectPath`,
`fullyQualifiedName`, `cwd`, `env`, `resultsLog`), the program-branch keys
(`program`, `args`, `request`, `process_id`), and the optional scope keys
(`scope`, `framework`, `caseArguments`, `methodParameters`). Keys beyond those
are **rejected, not ignored** (the same law `manual-attach.md` states for a
pid). The schema accepting a `scope` key does not make class scope a feature of
the debug path: what the shipped flow emits is the locator's decision, not the
schema's, and the locator ships exact test names only (see below).

Without a scope, the adapter treats `fullyQualifiedName` as one exact test and
filters with `FullyQualifiedName=<value>`. Put a class name there and the filter
becomes `FullyQualifiedName=Basic.AddTests` — an exact match against a class
selects **zero tests**, because no test is *named* `Basic.AddTests`; the tests live
*under* it (`Basic.AddTests.AddsOneAndTwo`, …). Copying a class-shaped config into
`debug.json` therefore gives you a session that runs nothing, not a whole class.
And a hand-typed `scope` is no escape either: a set selector hands the debugger
whichever host the runtime started first — not the test you meant — which is the
wrong-set failure the exact-FQN rule exists to prevent.

## How a class actually runs

Nothing in this extension carries a class scope. A class runs through a task:
Zed's gutter class **Run** action invokes `dotnet test . --filter` with a substring
filter over the class, with a trailing dot so it stops at the class and not at a
sibling whose name shares the prefix:

```text
FullyQualifiedName~Basic.AddTests.
```

That `~` substring filter is exactly what the shipped `crates/zatara/languages/csharp/tasks.json`
writes for its "Run class" gutter action, and it goes straight to `dotnet test` —
no locator and no debugger in the path. A plain run with no debugger attached is
allowed to select a set. The zatara debug path is not: its locator recognises a
gutter action by the exact `FullyQualifiedName=` filter alone (`FILTER_PREFIX`), and
the class action ships a substring filter in that same env, so the locator builds
no scenario and starts no zatara debug session for a class. The README's Debug
scenario keys section states the reason: a debugger attached to whichever host the
runtime happened to start first is not debugging the test you meant.

## What to do instead

- **To run every test in a class**, click the gutter `>` on the class and pick Run
  — that task is the only shipped path that selects a set.
- **To debug one test**, start from
  [`gutter-test.debug.json`](gutter-test.debug.json) with the test's full name —
  `Basic.AddTests.AddsOneAndTwo`, not the class — or use the gutter Debug action on
  a single method.
- **To attach to a host you started yourself**, follow [`manual-attach.md`](manual-attach.md).

> The schema now mirrors the full launch-config contract, `scope` key included —
> schema keys equal serde fields is the shipped equality law. Mirroring keys is
> not shipping features: nothing in the shipped gutter flow emits a scope, the
> exact-FQN rule still governs the debug path, and the class **Run** task above
> remains the shipped way to run a class.
