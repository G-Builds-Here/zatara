# Upstream backlog

Rough edges zatara runs into in somebody else's project, recorded here instead
of on their issue tracker. This is the whole record: the list lives in this
repository, and it is what zatara *would* file, with the evidence it would
attach, once the owner opens the gate.

## Filing policy

**Zero issues and zero pull requests have been opened against any third-party
repository.** Not one, including the friendly ones.

The owner's maturity verdict gates third-party filing on this project. Until
that verdict lands, upstream gaps are documented here and worked around in this
repository. The project is not published to the Zed marketplace and its binaries
are not distributed by anyone else, so nothing upstream needs a notice, and a
filing against someone's tracker is a public commitment this project is not
ready to defend.

Contributors: do not open one on our behalf either. A gap in this list is
known-good information, not an oversight waiting for a helpful stranger.

## Known gaps

| Gap | Upstream owner | Status |
|---|---|---|
| Test-run completion affordance in the debug panel | Zed (debug panel) | Not filed — owner gate. The proxy already emits `exited` and `terminated`, so the missing piece is purely Zed-side presentation of a finished run. |
| C# language-asset ownership and coordination | Zed (C# language assets) | Not filed — owner gate. zatara ships its own `runnables.scm` + `tasks.json` (DR-5) because coordination with the external C# assets is unavailable while filing is withheld. |
| Locator ergonomics for a scenario without a build task | Zed (task and locator model) | Not filed — owner gate. `build: None` is what ships; a static echo build task is the committed fallback, kept because core has never been observed firing a locator with no build step. |
| Per-OS command selection in extension-contributed tasks | Zed (task templates) | Not filed — owner gate. Zed's task schema drops an `os` key silently, so one task cannot carry both a Windows and a POSIX command; live ParserError, 2026-10-14. |
| Multiple task choices at one gutter position | Zed (task and gutter model) | Not filed — owner gate. One position resolves one `TaskContext` (one value per variable) and one template to one task, so same position + tag keeps only the last match; live measurement 2026-10-21. |

## What each gap costs a user today

**Test-run completion affordance.** When a debug session that started from a gutter
click finishes, Zed's debug panel has no "this run finished, and here is whether
the test passed" state — the answer is in the Console tab (the vstest `Passed!`
summary), and the panel's own end signal is the session tearing down. zatara
already reports the lifecycle a completion affordance would key off: the proxy
relays `exited` and `terminated` from the real process tree, and it does not
invent them. Nothing in an extension can manufacture the panel state, which is
why this gap is listed and not fixed.

**C# language-asset coordination.** Gutter icons for .NET tests come from
`crates/zatara/languages/csharp/runnables.scm` and
`crates/zatara/languages/csharp/tasks.json`, shipped by
zatara. That is a deliberate ownership decision (epic decision DR-5), not an
accident: the exact-FQN filter that makes a debug session run the test you
clicked is composed from tree-sitter captures that have to agree with the task
and the locator, and a third party's asset file is not something this repo can
keep that contract against. The cost is overlap — an install that also carries
another C# extension can offer two sets of gutter affordances for the same
buffer. The ask, when it is filed, is a coordination story for who owns .NET
test runnables in Zed, with the tag-string join key
(`csharp-test-method` / `csharp-test-class`) as the concrete example.

**Locator ergonomics without a build task.** A gutter debug click reaches the
locator through a task, and the scenario zatara emits declares `build: None`
because the proxy runs `dotnet build` itself, inside the pty whose output the
user watches. There is no first-class notion of "debug this test, there is no
build step" — the build step is where Zed's task model carries context into a
locator, so zatara carries the test identity in a task environment variable
(`ZATARA_TEST_FILTER`) instead. The fallback shipped alongside it is a build task
that echoes one fixed line and resolves nothing, held in reserve against a core
that refuses to fire a locator with no build task at all. That refusal has never
been observed either way in a live editor (the adapter and locator are not yet
declared to Zed — see the extension's Status section), so the fallback stays
committed rather than deleted.

**Per-OS command selection in extension tasks.** A task's `command`/`args` are
one string joined and handed to the user's system shell — PowerShell on
Windows, a POSIX shell elsewhere. Any command needing folder logic therefore
needs two scripts, one per shell family, and Zed 1.23.3's `TaskTemplate`
offers no way to pick between them: an `os` key in `tasks.json` is not a
schema error, it is silently dropped, and the POSIX leg runs on Windows
(verified by direct read of `crates/task/src/task_template.rs`, and live by a
PowerShell `ParserError` on a shipped walker on 2026-10-14). zatara's Run lane
today carries zero shell logic — bare `dotnet test "<worktree root>"` argv —
and delegates project choice to dotnet itself; the richer per-folder rule the
Debug lane applies could not be expressed in a task without this gap closing.
The ask, when filed: honor per-OS command/args in task templates (or expose
the resolved shell so an extension can pick a command that matches it), with
the silent-drop of unknown keys as the concrete failure mode to fix.

**Multiple task choices at one gutter position.** A `[MemberData]`/`[ClassData]`
theory would ideally carry one gutter icon listing one run option per source row,
the way each `[InlineData]` line carries its own. Zed cannot show it: a position's
context is a single `TaskContext` — one value per variable — and one template
resolves to one task per context, so N row-matches at one position collapse to the
last one (measured live 2026-10-21: only the final row of a three-row source was
selectable). zatara therefore runs the whole theory from the test method's own icon and
points per-row targeting at the manual-attach template (owner ruling
2026-10-21: no icons on data-definition lines — a row inside another object
reads as a test it is not). The ask, when
filed: let one position offer N resolved variants of one template (per captured
runnable), with the last-match collapse as the concrete failure mode to fix.

## Keeping this list honest

The test suite reads this file: `crates/zatara/tests/upstream_backlog.rs`
requires a row for each gap above, a non-empty one-line status per row that
answers the filing question, and it fails if any shipped markdown document links
an issue or pull request on anyone's tracker — this repository's included. Add a
gap by adding a row; the guard is what stops the list from quietly becoming a
paragraph.
