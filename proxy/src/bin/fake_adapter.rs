//! Scripted stand-in for a netcoredbg adapter child (test infrastructure —
//! never built for shipping targets, see `required-features` in Cargo.toml).
//!
//! Speaks the same Content-Length framed DAP over stdio that netcoredbg
//! speaks in `--interpreter=vscode` mode, replaying the spec-modelled bodies
//! from `fixtures/dap/` so the relay can be asserted field-for-field in CI
//! with no netcoredbg binary present (fixture provenance: that directory's
//! README — these are DAP-spec models, not live captures).
//!
//! Script controls (env, read once at startup):
//! - `FAKE_EXIT_ON_CMD=<command>`   — exit before answering that request
//!   (child dies with a request in flight).
//! - `FAKE_EXIT_AFTER_CMD=<command>` — exit right after answering it.
//! - `FAKE_EXIT_CODE=<n>`           — exit code for both (default 0).
//! - `FAKE_ECHO_ARGS=<command>`     — answer that request with its own
//!   arguments as the response body (child-side echo: proves what actually
//!   arrived through the relay, after envelope rewriting).
//! - `FAKE_STOPPED_REASON=<reason>` — the `stopped` event's reason (default
//!   `step`; a gutter breakpoint session scripts `breakpoint`).
//! - `FAKE_ENDED_AFTER_CMD=<command>` — 200 ms after answering that request,
//!   emit the debuggee-done pair (`exited` then `terminated`) and KEEP
//!   SERVING: what a real netcoredbg does when its debuggee dies (repro6
//!   probe 2026-10-13), scripted so the settle gate's ordering — summary
//!   before session-end — is assertable without a real testhost.
//! - `FAKE_ARGV_LOG=<file>`         — record argv (one arg per line) before
//!   serving: the child-side truth of what the relay actually spawned with.
//!
//! Adapter argv honored (netcoredbg fidelity — LOC-0117):
//! - `--engineLogging=<file>`       — real netcoredbg dumps the full DAP
//!   wire traffic to that file as `<-`/`->` JSONL when given this flag
//!   (research findings §4); the fake reproduces exactly that, so an enabled
//!   engine-logging gate can be asserted on the file's existence and shape
//!   without a netcoredbg binary. Unhonored flags are still ignored, as with
//!   every other argv the proxy today forwards.
//!
//! Scripted event stream (mirrors what a real netcoredbg session emits):
//! `setBreakpoints` → `breakpoint` event; `configurationDone` → `stopped`
//! event; `continue` → `continued` event; `next` → `output` event. A
//! `disconnect` request is answered, then the fake exits — real netcoredbg
//! terminates its interpreter after processing disconnect.
//!
//! The framing codec here is deliberately local: test-harness code stays
//! self-contained so a production-codec bug can never mask itself with an
//! identical bug in the harness. The production codec lives in the proxy lib.

use std::env;
use std::io::{self, BufRead, BufReader, Write};
use serde_json::{json, Value};

const SET_BREAKPOINTS_BODY: &str =
    include_str!("../../../../fixtures/dap/set_breakpoints_response_body_unknown_field.json");
const OUTPUT_BODY: &str = include_str!("../../../../fixtures/dap/output_event_body.json");
const BREAKPOINT_BODY: &str = include_str!("../../../../fixtures/dap/breakpoint_event_body.json");

/// Read one `Content-Length` framed body and parse it as JSON.
fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut header: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte)?;
        if n == 0 {
            return Ok(None);
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") || header.ends_with(b"\n\n") {
            break;
        }
        if header.len() > 8192 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "header block too long"));
        }
    }
    let mut length: Option<usize> = None;
    for line in String::from_utf8_lossy(&header).split('\n') {
        if let Some((key, value)) = line.trim().split_once(':') {
            if key.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let length = length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no Content-Length"))?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?))
}

fn write_frame(out: &mut impl Write, message: &Value) {
    let body = message.to_string();
    write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    out.flush().unwrap();
}

/// Send a frame and mirror it into the engine log (the `<-`/`->` JSONL real
/// netcoredbg writes for `--engineLogging`; the fake owes the same shape —
/// Mock Fidelity Ladder).
fn emit(out: &mut impl Write, engine_log: &mut Option<std::fs::File>, message: &Value) {
    write_frame(out, message);
    if let Some(log) = engine_log {
        writeln!(log, "-> {message}").expect("fake adapter: engine log write");
    }
}

fn main() {
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(spy) = env::var("FAKE_ARGV_LOG") {
        let mut file = std::fs::File::create(&spy).expect("fake adapter: argv spy creatable");
        for arg in &argv {
            writeln!(file, "{arg}").expect("fake adapter: argv spy writable");
        }
    }
    // Netcoredbg fidelity (LOC-0117): the engine-logging flag means wire
    // traffic starts landing in that file this instant (create+truncate, as
    // the real adapter opens its engine log per run).
    let mut engine_log: Option<std::fs::File> = argv
        .iter()
        .find_map(|arg| arg.strip_prefix("--engineLogging="))
        .map(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)
                .expect("fake adapter: engine log creatable")
        });

    let exit_on = env::var("FAKE_EXIT_ON_CMD").unwrap_or_default();
    let exit_after = env::var("FAKE_EXIT_AFTER_CMD").unwrap_or_default();
    let exit_code: i32 = env::var("FAKE_EXIT_CODE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let echo_args = env::var("FAKE_ECHO_ARGS").unwrap_or_default();
    let stopped_reason = env::var("FAKE_STOPPED_REASON").unwrap_or_else(|_| "step".into());
    let ended_after = env::var("FAKE_ENDED_AFTER_CMD").unwrap_or_default();

    let mut seq = 1i64;
    while let Some(msg) = read_frame(&mut reader).expect("fake adapter: framing error") {
        if let Some(log) = &mut engine_log {
            writeln!(log, "<- {msg}").expect("fake adapter: engine log write");
        }
        let command = msg.get("command").and_then(Value::as_str).unwrap_or("").to_string();
        let request_seq = msg.get("seq").and_then(Value::as_i64).unwrap_or(0);

        if !exit_on.is_empty() && exit_on == command {
            std::process::exit(exit_code); // die mid-request: no response, no events
        }

        let body: Value = if !echo_args.is_empty() && echo_args == command {
            msg.get("arguments").cloned().unwrap_or(Value::Null)
        } else {
            match command.as_str() {
                "initialize" => json!({ "capabilities": { "supportsConfigurationDoneRequest": true } }),
                "setBreakpoints" => {
                    serde_json::from_str(SET_BREAKPOINTS_BODY).expect("setBreakpoints fixture body")
                }
                _ => json!({}),
            }
        };
        emit(
            &mut out,
            &mut engine_log,
            &json!({
                "seq": seq, "type": "response", "request_seq": request_seq,
                "command": command, "success": true, "body": body,
            }),
        );
        seq += 1;

        match command.as_str() {
            "setBreakpoints" => {
                // A real netcoredbg announces binding with a `breakpoint`
                // event (reason new/bound) carrying the Breakpoint body —
                // the forward-compat fixture body proves undeclared fields
                // survive the relay hop on the event path too.
                emit(
                    &mut out,
                    &mut engine_log,
                    &json!({ "seq": seq, "type": "event", "event": "breakpoint",
                             "body": serde_json::from_str::<Value>(BREAKPOINT_BODY)
                                 .expect("breakpoint fixture body") }),
                );
                seq += 1;
            }
            "configurationDone" => {
                emit(
                    &mut out,
                    &mut engine_log,
                    &json!({ "seq": seq, "type": "event", "event": "stopped",
                             "body": { "reason": stopped_reason, "threadId": 1 } }),
                );
                seq += 1;
            }
            "continue" => {
                emit(
                    &mut out,
                    &mut engine_log,
                    &json!({ "seq": seq, "type": "event", "event": "continued",
                             "body": { "threadId": 1 } }),
                );
                seq += 1;
            }
            "next" => {
                emit(
                    &mut out,
                    &mut engine_log,
                    &json!({ "seq": seq, "type": "event", "event": "output",
                             "body": serde_json::from_str::<Value>(OUTPUT_BODY)
                                 .expect("output fixture body") }),
                );
                seq += 1;
            }
            _ => {}
        }

        if !ended_after.is_empty() && ended_after == command {
            // The debuggee dying under a live interpreter: netcoredbg
            // announces `exited` then `terminated` and keeps serving
            // (repro6 probe 2026-10-13) — the pair leaves here while the
            // runner's test step may still be printing its summary. The
            // pause makes the pair arrive FIRST on a proxy without the
            // settle gate, so the regression test fails without the fix.
            std::thread::sleep(std::time::Duration::from_millis(200));
            emit(
                &mut out,
                &mut engine_log,
                &json!({ "seq": seq, "type": "event", "event": "exited",
                         "body": { "exitCode": 0 } }),
            );
            seq += 1;
            emit(
                &mut out,
                &mut engine_log,
                &json!({ "seq": seq, "type": "event", "event": "terminated" }),
            );
            seq += 1;
        }

        if command == "disconnect" {
            std::process::exit(0); // real netcoredbg terminates after disconnect
        }
        if !exit_after.is_empty() && exit_after == command {
            std::process::exit(exit_code);
        }
    }
}
