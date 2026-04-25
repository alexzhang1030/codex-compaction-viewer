use codex_compaction_viewer::tui::{build_tui_model, handle_key, TuiFocus, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

fn write_session_without_compaction(path: &Path, session_id: &str, timestamp: &str, cwd: &str) {
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
            "payload": {"type": "user_message", "message": "Regular session"}
        }),
        json!({
            "timestamp": timestamp,
            "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "content": "No compaction here"}
        }),
    ];
    let mut file = fs::File::create(path).expect("create fixture");
    for row in rows {
        writeln!(file, "{row}").expect("write row");
    }
}

fn write_mixed_session(path: &Path) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create session dir");
    let rows = vec![
        json!({
            "timestamp": "2026-04-25T12:00:00Z",
            "type": "session_meta",
            "payload": {"id": "mixed-session", "cwd": "/repo/mixed"}
        }),
        json!({
            "timestamp": "2026-04-25T12:00:01Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Run the tests"}]
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:00:02Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {"total_token_usage": {"total_tokens": 1200}}
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:00:03Z",
            "type": "response_item",
            "payload": {
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "Internal chain summary"}]
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:00:04Z",
            "type": "turn_context",
            "payload": {
                "turn_id": "mixed-turn",
                "summary": "Compacted context to keep.",
                "truncation_policy": {"mode": "tokens", "limit": 12000}
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:00:05Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Tests passed"}]
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:00:06Z",
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "exec_command",
                "call_id": "call-1",
                "arguments": "{\"cmd\":\"cargo test\"}"
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:00:07Z",
            "type": "response_item",
            "payload": {
                "type": "function_call_output",
                "call_id": "call-1",
                "output": "ok"
            }
        }),
        json!({
            "timestamp": "2026-04-25T12:00:08Z",
            "type": "event_msg",
            "payload": {"type": "turn_aborted", "message": "stop"}
        }),
        json!({
            "timestamp": "2026-04-25T12:00:09Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "system",
                "content": [{"type": "output_text", "text": "hidden in tidy"}]
            }
        }),
    ];
    let mut file = fs::File::create(path).expect("create fixture");
    for row in rows {
        writeln!(file, "{row}").expect("write row");
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn visible_history_lines(state: &mut TuiState) -> Vec<usize> {
    state.selected_message = 0;
    let mut lines = Vec::new();
    loop {
        let Some(line) = state.selected_message_line() else {
            break;
        };
        lines.push(line);
        handle_key(state, key(KeyCode::Down));
        if state.selected_message_line() == Some(line) {
            break;
        }
    }
    lines
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
fn tui_state_filters_sessions_by_project_search_text() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(
        &tmp.path().join("sessions/2026/04/25/rollout-nav.jsonl"),
        "nav-session",
        "2026-04-25T13:00:00Z",
        "/work/projects/navigation-stack",
        "Navigation compaction.",
    );
    write_session(
        &tmp.path().join("sessions/2026/04/25/rollout-ops.jsonl"),
        "ops-session",
        "2026-04-25T12:00:00Z",
        "/work/projects/ops-dashboard",
        "Ops compaction.",
    );

    let model = build_tui_model(Some(tmp.path()), false, None).expect("build model");
    let mut state = TuiState::new(model);

    state.set_session_search("project:NAVIGATION");

    assert_eq!(state.visible_session_ids(), vec!["nav-session"]);
    assert_eq!(state.current_session_id(), Some("nav-session"));
}

#[test]
fn tui_state_filters_sessions_by_compaction_tag() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(
        &tmp.path().join("sessions/2026/04/25/rollout-compact.jsonl"),
        "compact-session",
        "2026-04-25T13:00:00Z",
        "/work/projects/compact",
        "Compacted context.",
    );
    write_session_without_compaction(
        &tmp.path().join("sessions/2026/04/25/rollout-plain.jsonl"),
        "plain-session",
        "2026-04-25T14:00:00Z",
        "/work/projects/plain",
    );

    let model = build_tui_model(Some(tmp.path()), false, None).expect("build model");
    let mut state = TuiState::new(model);

    state.set_session_search("tag:compaction");

    assert_eq!(state.visible_session_ids(), vec!["compact-session"]);
    assert_eq!(state.current_session_id(), Some("compact-session"));
}

#[test]
fn tui_keybindings_update_session_search_and_compaction_filter() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(
        &tmp.path().join("sessions/2026/04/25/rollout-nav.jsonl"),
        "nav-session",
        "2026-04-25T13:00:00Z",
        "/work/projects/nav",
        "Navigation compaction.",
    );
    write_session_without_compaction(
        &tmp.path().join("sessions/2026/04/25/rollout-plain.jsonl"),
        "plain-session",
        "2026-04-25T14:00:00Z",
        "/work/projects/plain",
    );

    let model = build_tui_model(Some(tmp.path()), false, None).expect("build model");
    let mut state = TuiState::new(model);

    handle_key(&mut state, key(KeyCode::Char('/')));
    handle_key(&mut state, key(KeyCode::Char('n')));
    handle_key(&mut state, key(KeyCode::Char('a')));
    handle_key(&mut state, key(KeyCode::Char('v')));
    handle_key(&mut state, key(KeyCode::Enter));

    assert_eq!(state.visible_session_ids(), vec!["nav-session"]);
    assert_eq!(state.focus(), TuiFocus::History);

    state.set_session_search("");
    handle_key(&mut state, key(KeyCode::Char('g')));

    assert!(state.compaction_session_filter_enabled());
    assert_eq!(state.visible_session_ids(), vec!["nav-session"]);
}

#[test]
fn tui_search_mode_accepts_q_as_search_text() {
    let tmp = TempDir::new().expect("tempdir");
    write_session(
        &tmp.path().join("sessions/2026/04/25/rollout-q.jsonl"),
        "query-session",
        "2026-04-25T13:00:00Z",
        "/work/projects/query-target",
        "Query compaction.",
    );

    let model = build_tui_model(Some(tmp.path()), false, None).expect("build model");
    let mut state = TuiState::new(model);

    handle_key(&mut state, key(KeyCode::Char('/')));
    let quit = handle_key(&mut state, key(KeyCode::Char('q')));

    assert!(!quit);
    assert_eq!(state.session_search(), "q");
    assert_eq!(state.visible_session_ids(), vec!["query-session"]);
}

#[test]
fn tui_defaults_to_tidy_history_and_toggles_verbose_mode() {
    let tmp = TempDir::new().expect("tempdir");
    write_mixed_session(&tmp.path().join("sessions/2026/04/25/rollout-mixed.jsonl"));

    let model = build_tui_model(Some(tmp.path()), false, None).expect("build model");
    let mut state = TuiState::new(model);

    assert_eq!(visible_history_lines(&mut state), vec![2, 5, 6, 7, 8]);

    handle_key(&mut state, key(KeyCode::Char('v')));

    assert_eq!(
        visible_history_lines(&mut state),
        vec![2, 3, 4, 5, 6, 7, 8, 9, 10]
    );
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

#[test]
fn tui_enter_focuses_detail_then_jk_scroll_detail_instead_of_history() {
    let tmp = TempDir::new().expect("tempdir");
    let session = tmp.path().join("sessions/2026/04/25/rollout-detail.jsonl");
    write_session(
        &session,
        "detail-session",
        "2026-04-25T12:00:00Z",
        "/repo/detail",
        "The retained compacted context for the selected session.",
    );

    let model = build_tui_model(Some(tmp.path()), false, None).expect("build model");
    let mut state = TuiState::new(model);
    let initial_line = state.selected_message_line();

    handle_key(&mut state, key(KeyCode::Enter));
    assert_eq!(state.focus(), TuiFocus::Detail);

    handle_key(&mut state, key(KeyCode::Char('j')));
    assert_eq!(state.selected_message_line(), initial_line);
    assert_eq!(state.detail_scroll(), 1);

    handle_key(&mut state, key(KeyCode::Char('k')));
    assert_eq!(state.selected_message_line(), initial_line);
    assert_eq!(state.detail_scroll(), 0);
}
