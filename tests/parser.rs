use codex_compaction_viewer::parser::{discover_sessions, parse_jsonl};
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

fn write_jsonl(path: &Path, rows: Vec<serde_json::Value>) {
    let mut file = fs::File::create(path).expect("create fixture");
    for row in rows {
        if let Some(raw) = row.as_str() {
            writeln!(file, "{raw}").expect("write raw row");
        } else {
            writeln!(file, "{row}").expect("write json row");
        }
    }
}

#[test]
fn parse_jsonl_extracts_codex_context_summary_event() {
    let tmp = TempDir::new().expect("tempdir");
    let session = tmp.path().join("rollout-2026-04-25T12-00-00-example.jsonl");
    write_jsonl(
        &session,
        vec![
            json!({
                "timestamp": "2026-04-25T12:00:00Z",
                "type": "session_meta",
                "payload": {
                    "id": "sess-1",
                    "cwd": "/repo",
                    "cli_version": "0.1.0",
                    "model_provider": "openai"
                }
            }),
            json!("not json"),
            json!({
                "timestamp": "2026-04-25T12:01:00Z",
                "type": "turn_context",
                "payload": {
                    "turn_id": "turn-1",
                    "model": "gpt-test",
                    "summary": "Prior work has been compacted into this summary.",
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
                        "total_token_usage": {
                            "input_tokens": 1200,
                            "cached_input_tokens": 200,
                            "output_tokens": 300,
                            "reasoning_output_tokens": 40,
                            "total_tokens": 1540
                        }
                    }
                }
            }),
            json!({
                "timestamp": "2026-04-25T12:01:07Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Done"}]
                }
            }),
        ],
    );

    let parsed = parse_jsonl(&session).expect("parse session");

    assert_eq!(parsed.metadata.session_id, "sess-1");
    assert_eq!(parsed.metadata.cwd, "/repo");
    assert_eq!(parsed.stats.bad_lines, 1);
    assert_eq!(parsed.stats.line_count, 5);
    assert_eq!(parsed.stats.total_tokens, 1540);
    assert_eq!(parsed.stats.model_context_window, 258400);
    assert_eq!(parsed.compaction_events.len(), 1);
    let event = &parsed.compaction_events[0];
    assert_eq!(event.turn_id, "turn-1");
    assert_eq!(event.summary_length(), 48);
    assert_eq!(event.truncation_mode, "tokens");
    assert_eq!(event.truncation_limit, Some(10000));
    assert!(event.summary.contains("Prior work"));
    assert!(parsed.messages.len() >= 3);
}

#[test]
fn discover_sessions_finds_active_and_archived_sessions() {
    let tmp = TempDir::new().expect("tempdir");
    let active = tmp.path().join("sessions/2026/04/25");
    let archived = tmp.path().join("archived_sessions");
    fs::create_dir_all(&active).expect("create active");
    fs::create_dir_all(&archived).expect("create archived");
    let active_file = active.join("rollout-active.jsonl");
    let archived_file = archived.join("rollout-archived.jsonl");
    fs::write(&active_file, "{}").expect("write active");
    fs::write(&archived_file, "{}").expect("write archived");

    let active_only = discover_sessions(Some(tmp.path()), false).expect("discover active");
    let with_archived = discover_sessions(Some(tmp.path()), true).expect("discover archived");

    assert_eq!(active_only, vec![active_file.clone()]);
    assert_eq!(with_archived, vec![archived_file, active_file]);
}

#[test]
fn parse_jsonl_ignores_legacy_auto_summary_mode() {
    let tmp = TempDir::new().expect("tempdir");
    let session = tmp.path().join("legacy.jsonl");
    write_jsonl(
        &session,
        vec![json!({
            "timestamp": "2026-04-25T12:00:00Z",
            "type": "turn_context",
            "payload": {
                "turn_id": "legacy-turn",
                "summary": "auto",
                "truncation_policy": {"mode": "tokens", "limit": 10000}
            }
        })],
    );

    let parsed = parse_jsonl(&session).expect("parse session");

    assert!(parsed.compaction_events.is_empty());
}

#[test]
fn parse_jsonl_pairs_raw_boundary_with_following_compact_summary() {
    let tmp = TempDir::new().expect("tempdir");
    let session = tmp.path().join("claude-style.jsonl");
    write_jsonl(
        &session,
        vec![
            json!({
                "timestamp": "2026-04-25T13:00:00Z",
                "type": "system",
                "subtype": "compact_boundary",
                "compactMetadata": {
                    "trigger": "auto",
                    "preCompactTokens": 15700
                }
            }),
            json!({
                "timestamp": "2026-04-25T13:00:02Z",
                "type": "user",
                "isCompactSummary": true,
                "message": {
                    "content": [
                        {"type": "text", "text": "The previous conversation was compacted into this summary."}
                    ]
                }
            }),
            json!({
                "timestamp": "2026-04-25T13:00:03Z",
                "type": "assistant",
                "message": {
                    "content": [{"type": "text", "text": "Ready to continue."}]
                }
            }),
        ],
    );

    let parsed = parse_jsonl(&session).expect("parse session");

    assert_eq!(parsed.stats.line_count, 3);
    assert_eq!(parsed.stats.bad_lines, 0);
    assert_eq!(parsed.compaction_events.len(), 1);
    let event = &parsed.compaction_events[0];
    assert_eq!(event.source, "boundary_summary");
    assert_eq!(event.line_number, 2);
    assert_eq!(event.boundary_line_number, Some(1));
    assert_eq!(event.trigger, "auto");
    assert_eq!(event.token_usage.as_ref().unwrap().total_tokens, 15700);
    assert!(event.summary.contains("previous conversation"));
}
