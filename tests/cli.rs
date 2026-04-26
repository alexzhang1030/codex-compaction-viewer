use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_session(path: &Path) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create session dir");
    let rows = vec![
        json!({
            "timestamp": "2026-04-25T12:00:00Z",
            "type": "session_meta",
            "payload": {"id": "sess-cli", "cwd": "/workspace/demo"}
        }),
        json!({
            "timestamp": "2026-04-25T12:01:00Z",
            "type": "turn_context",
            "payload": {
                "turn_id": "turn-cli",
                "summary": "A compacted context snapshot for CLI tests.",
                "truncation_policy": {"mode": "tokens", "limit": 10000}
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:01:05Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model_context_window": 258400,
                    "total_token_usage": {"total_tokens": 42}
                }
            }
        }),
    ];
    let mut file = fs::File::create(path).expect("create fixture");
    for row in rows {
        writeln!(file, "{row}").expect("write row");
    }
}

fn cxv() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cxv"))
}

#[test]
fn version_flags_print_package_version() {
    for flag in ["--version", "-v"] {
        let output = cxv().arg(flag).output().expect("run cxv");

        assert!(
            output.status.success(),
            "{flag} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("cxv {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn scan_json_outputs_structured_session_rows() {
    let tmp = TempDir::new().expect("tempdir");
    let session = tmp.path().join("sessions/2026/04/25/rollout-cli.jsonl");
    write_session(&session);

    let output = cxv()
        .args(["--scan", "--root", tmp.path().to_str().unwrap(), "--json"])
        .output()
        .expect("run cxv");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse scan output");
    assert_eq!(rows[0]["session_id"], "sess-cli");
    assert_eq!(rows[0]["compactions"], 1);
    assert_eq!(rows[0]["total_tokens"], 42);
    assert_eq!(rows[0]["cwd"], "/workspace/demo");
}

#[test]
fn summary_outputs_context_summary_details() {
    let tmp = TempDir::new().expect("tempdir");
    let session = tmp.path().join("rollout-cli.jsonl");
    write_session(&session);

    let output = cxv()
        .args(["--summary", session.to_str().unwrap()])
        .output()
        .expect("run cxv");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("turn-cli"));
    assert!(stdout.contains("A compacted context snapshot"));
    assert!(stdout.contains("tokens:10000"));
}

#[test]
fn tui_mode_option_accepts_verbose() {
    let output = cxv()
        .args(["--mode", "verbose", "--tui"])
        .output()
        .expect("run cxv");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Interactive TUI requires a terminal"));
}

#[test]
fn tui_raw_bodies_flag_is_accepted() {
    let output = cxv()
        .args(["--tui", "--raw-bodies"])
        .output()
        .expect("run cxv");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Interactive TUI requires a terminal"));
}

#[test]
fn tui_no_mouse_flag_is_accepted() {
    let output = cxv()
        .args(["--tui", "--no-mouse"])
        .output()
        .expect("run cxv");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Interactive TUI requires a terminal"));
}

#[test]
fn invalid_args_exit_with_parse_error() {
    let output = cxv().arg("--no-such-flag").output().expect("run cxv");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument"));
}

#[test]
fn missing_file_exits_with_runtime_error() {
    let output = cxv()
        .args(["--summary", "/definitely/missing/session.jsonl"])
        .output()
        .expect("run cxv");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to open"));
}

#[test]
fn summary_handles_empty_and_multi_event_sessions() {
    let tmp = TempDir::new().expect("tempdir");
    let empty_session = tmp.path().join("empty.jsonl");
    write_session(&empty_session);
    fs::write(
        &empty_session,
        "{\"timestamp\":\"2026-04-25T12:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"hello\"}}\n",
    )
    .expect("overwrite empty session");

    let empty_output = cxv()
        .args(["--summary", empty_session.to_str().unwrap()])
        .output()
        .expect("run empty summary");
    assert!(empty_output.status.success());
    assert!(String::from_utf8_lossy(&empty_output.stdout).contains("No Codex context summary events found."));

    let multi_session = tmp.path().join("multi.jsonl");
    let rows = vec![
        json!({
            "timestamp": "2026-04-25T12:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {"total_tokens": 2_500_000}
                }
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:01:00Z",
            "type": "turn_context",
            "payload": {
                "turn_id": "turn-one",
                "summary": "First summary",
                "truncation_policy": {"mode": "tokens", "limit": 100}
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:02:00Z",
            "type": "system",
            "subtype": "compact_boundary",
            "compactMetadata": {"trigger": "auto", "preCompactTokens": 42}
        }),
        json!({
            "timestamp": "2026-04-25T12:02:01Z",
            "type": "user",
            "isCompactSummary": true,
            "message": {"content": [{"type": "text", "text": "Second summary"}]}
        }),
    ];
    let mut file = fs::File::create(&multi_session).expect("create multi fixture");
    for row in rows {
        writeln!(file, "{row}").expect("write multi row");
    }

    let output = cxv()
        .args(["--summary", multi_session.to_str().unwrap()])
        .output()
        .expect("run multi summary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("#1 line 2 turn turn-one"));
    assert!(stdout.contains("#2 line 4 boundary 3"));
    assert!(stdout.contains("trigger: auto"));
    assert!(stdout.contains("tokens before: 2.5m"));
    assert!(stdout.contains("First summary"));
    assert!(stdout.contains("Second summary"));
}
