use codex_compaction_viewer::tui::{build_tui_model, TuiState};
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

fn write_session(path: &Path, session_id: &str, timestamp: &str, cwd: &str, summary: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create session dir");
    let rows = vec![
        json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {"id": session_id, "cwd": cwd}
        }),
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "Before compaction"}
        }),
        json!({
            "timestamp": timestamp,
            "type": "turn_context",
            "payload": {
                "turn_id": format!("{session_id}-turn"),
                "summary": summary,
                "truncation_policy": {"mode": "tokens", "limit": 12000}
            }
        }),
        json!({
            "timestamp": timestamp,
            "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "content": "After compaction"}
        }),
    ];
    let mut file = fs::File::create(path).expect("create fixture");
    for row in rows {
        writeln!(file, "{row}").expect("write row");
    }
}

#[test]
fn build_tui_model_loads_active_and_archived_sessions_newest_first() {
    let tmp = TempDir::new().expect("tempdir");
    let active = tmp.path().join("sessions/2026/04/24/rollout-old.jsonl");
    let archived = tmp
        .path()
        .join("archived_sessions/2026/04/25/rollout-new.jsonl");
    write_session(
        &active,
        "old-session",
        "2026-04-24T12:00:00Z",
        "/repo/old",
        "Old compacted context.",
    );
    write_session(
        &archived,
        "new-session",
        "2026-04-25T12:00:00Z",
        "/repo/new",
        "New compacted context.",
    );

    let model = build_tui_model(Some(tmp.path()), true, None).expect("build model");

    assert_eq!(model.sessions.len(), 2);
    assert_eq!(model.sessions[0].session_id, "new-session");
    assert_eq!(model.sessions[0].cwd, "/repo/new");
    assert_eq!(model.sessions[0].compactions, 1);
    assert_eq!(model.sessions[1].session_id, "old-session");
}

#[test]
fn tui_state_can_jump_to_compactions_and_render_summaries() {
    let tmp = TempDir::new().expect("tempdir");
    let session = tmp.path().join("sessions/2026/04/25/rollout-one.jsonl");
    write_session(
        &session,
        "one-session",
        "2026-04-25T12:00:00Z",
        "/repo/one",
        "The retained compacted context for the selected session.",
    );

    let model = build_tui_model(Some(tmp.path()), false, None).expect("build model");
    let mut state = TuiState::new(model);

    assert_eq!(state.selected_message_line(), Some(2));
    state.jump_next_compaction();
    assert_eq!(state.selected_message_line(), Some(3));
    state.jump_previous_compaction();
    assert_eq!(state.selected_message_line(), Some(3));

    let summaries = state.compaction_summary_text();
    assert!(summaries.contains("COMPACTION 1"));
    assert!(summaries.contains("line 3"));
    assert!(summaries.contains("The retained compacted context"));
}
