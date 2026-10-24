# Manual attach: drive a paused test host yourself

This is the escape hatch, not the normal path. Use it when you want a debugger
on a test run that this extension is not orchestrating: a test that only fails
under the conditions you can set up by hand, a host you started for another
reason, or a session where you want the debugger aimed at one pid you chose.

Two lanes reach that paused host, and they share its setup:

| Lane | You drive | The debugger runs in |
|------|-----------|----------------------|
| [Attach through Zed](#attach-through-zed-the-debug-panel-lane) | a hand-written `.zed/debug.json` entry + F5 | Zed's debug panel |
| [Attach without Zed](#attach-without-zed-the-raw-netcoredbg-cli-lane) | `netcoredbg --attach <pid>` | netcoredbg's own command line |

Either way the host must be paused first, and its pid is the number you name —
so the three shared steps come first.

## 1. Build the project

```sh
dotnet build path/to/Basic/Basic.csproj
```

## 2. Start the test host paused

vstest parks its host when `VSTEST_HOST_DEBUG=1` is in the environment — the
same switch the extension's proxy forces, and the only reason a gutter debug
session can attach at all. Run the filtered test with `--no-build` so nothing
rebuilds under you:

```powershell
# Windows PowerShell
$env:VSTEST_HOST_DEBUG = "1"
dotnet test path/to/Basic/Basic.csproj --no-build --filter "FullyQualifiedName=Basic.AddTests.AddsOneAndTwo"
```

```cmd
:: Windows cmd — set and run in one command line, no separate shell state
set VSTEST_HOST_DEBUG=1&& dotnet test path/to/Basic/Basic.csproj --no-build --filter "FullyQualifiedName=Basic.AddTests.AddsOneAndTwo"
```

```sh
# Linux / macOS
VSTEST_HOST_DEBUG=1 dotnet test path/to/Basic/Basic.csproj --no-build \
  --filter "FullyQualifiedName=Basic.AddTests.AddsOneAndTwo"
```

## 3. Read the pid from the pause banner

The paused host announces itself in the test output. The number after
`Process Id:` is the whole rendezvous — this is the exact banner the shipped
runner scans for:

```text
Testhost process for source settings file ... exited with error ...
Process Id: 34060, Name: testhost
```

The test run sits waiting at that point. Do not close the window; the pid is
your attach target, and it belongs to `testhost`, a child of `dotnet test`.

## Attach through Zed (the debug-panel lane)

Save this as `.zed/debug.json` in the project root — the whole file, array
brackets included (a bare object is not what the file format is):

```json
[
  {
    "label": "zatara attach",
    "adapter": "zatara-netcoredbg",
    "config": {
      "request": "attach",
      "process_id": 34060
    }
  }
]
```

Open Zed's debug panel: the entry lists under its `label`. Put the pid from
step 3 in `process_id` and press F5. Zed sends this config to the adapter as a
DAP `attach`, not a `launch`: the extension names the kind on the wire from the
config itself, and its pre-flight probe answers the same kind from the same
rule, so the gate and the wire cannot disagree. The debugger goes to the pid
you named; Zed never picks or launches a program of its own.

The proxy vouches for the pid with the OS before the debugger is aimed: a pid
that names no running process is answered a clean DAP error that names the pid
(netcoredbg's own attach-failure signature is `Error: 0x80070057` —
E_INVALIDARG — uniform for dead, non-CoreCLR, and privileged pids, and it names
nothing, so the shipped lane names the pid for you). Keys beyond the contract
are still rejected, not ignored. The build-pinned record below applies to this
lane too.

`process_id` is a snapshot: it changes with every test run, so the number goes
stale the moment the host exits. Edit the entry with a fresh pid before each
F5 — and pick the entry from the debug panel rather than the run menu, since
this scenario spawns no build task of its own.

## Attach without Zed (the raw netcoredbg CLI lane)

`--attach <process-id>` is netcoredbg's own documented flag. The shipped lane
authors the same call (the debugger's own `processId` key, from your
`process_id`) through the proxy, so the two paths meet at one rendezvous:

```sh
netcoredbg --attach 34060
```

You land in netcoredbg's command-line interpreter with the paused host attached.
vstest resumes the test as soon as a debugger attaches, so set breakpoints
before you let it run. netcoredbg's own documentation covers its command set.

If netcoredbg is not on your PATH: on Windows, Linux, and Apple-silicon
macOS the extension keeps a verified copy in its work-dir cache
(`binary-cache/`), and that path works here too — it is just a binary.
Intel macOS: upstream ships no Intel build, so it is a source build
(https://github.com/Samsung/netcoredbg/releases).

## Debug, then let it finish (both lanes)

Breakpoints do NOT bind in attach sessions on the build this extension pins
(netcoredbg 3.2.0-1092, measured live 2026-10): MI and CLI inserts
carry the warning "No executable code of the debugger's target code type is
associated with this line", DAP `setBreakpoints` answers `"verified":false`,
and zero hits were observed on either target shape. You can interrupt and
inspect (the CLI's `interrupt` stops all threads, and thread state answers
while stopped), but do not plan a breakpoint-driven session on this build:
the refusal was measured even against a paused testhost over the same
DAP surface the extension relays.
Press continue to run the test to completion; the `Passed!` summary and the
process exit appear in the terminal that owns the `dotnet test` window, not in
the debugger's.

## What this path cannot give you

- **No test output in the debugger's console.** A debugger cannot relay a
  console that was already bound before it arrived, so `dotnet test`'s output
  stays in its own terminal. This limitation is precisely why the extension owns
  the pty in the first place: the proxy is there from the start, so it can put
  that same output into Zed's Console.
- **A paused `testhost` does not survive the session.** The resume that
  configures a session releases the paused test to run: it completes and its
  host exits with it — measured live 2026-10 and again on the shipped
  lane with the pinned real adapter (2026-10-16). Plan for the host to die
  with the session.
- **A Zed session stop KILLS the attached-to process too.** A stop carrying
  `terminateDebuggee: true` kills within seconds (measured live, reproduced on
  the shipped lane). The Zed stop is not the flagless `disconnect` an earlier
  reading assumed: Zed sends DAP `terminate` (`{"restart": false}`) and netcoredbg
  honors it — the attached-to process died within a second, 2/2 live Zed 1.24.2
  runs on 2026-10-20, the request wire-captured at the proxy→engine relay. zatara
  relays what it is given and invents nothing: no synthesized detach, no hidden
  kill; the kill here is Zed's chosen request, not the relay's. To keep a process
  attached-to by choice, use the debugger's own explicit detach (netcoredbg CLI
  `detach`, MI `-target-detach`) before the session ends.
- **A debugger that dies mid-session takes the attached-to process with it.**
  The OS debug stack tears the target down when its debugger goes while still
  attached (verified live) — a crashed netcoredbg is not a detach. A surviving
  `testhost` from a deliberate detach still locks build outputs — the next
  `dotnet build` fails with MSB3026/MSB3027 until it is gone (`taskkill /PID
  <pid> /T /F` on Windows, `kill <pid>` elsewhere).
- **No exact-FQN guarantee from a substring filter.** Same rule as the gutter:
  `FullyQualifiedName=` matches one test; `FullyQualifiedName~` selects a set,
  and attaching to whichever host vstest happened to start first is not
  debugging what you meant.
