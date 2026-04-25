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
