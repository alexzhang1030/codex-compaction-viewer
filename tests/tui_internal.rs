pub use codex_compaction_viewer::{parser, APP_NAME, APP_VERSION};

mod tui_internal {
    include!("../src/tui.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::parser::{
            CompactionEvent, ConversationStats, ParsedMessage, ParsedSession, SessionMetadata,
            TokenUsage,
        };
        use ratatui::backend::TestBackend;
        use serde_json::json;
        use std::fs;
        use std::io::Write;
        use std::path::Path;
        use tempfile::TempDir;

        fn write_rows(path: &Path, rows: Vec<serde_json::Value>) {
            fs::create_dir_all(path.parent().expect("parent")).expect("create session dir");
            let mut file = fs::File::create(path).expect("create fixture");
            for row in rows {
                writeln!(file, "{row}").expect("write row");
            }
        }

        fn render_state(state: &TuiState, width: u16, height: u16) -> String {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal.draw(|frame| draw(frame, state)).expect("draw");
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        }

        fn model_with_session(tmp: &TempDir, rows: Vec<serde_json::Value>) -> TuiModel {
            let session = tmp.path().join("sessions/2026/04/25/rollout-render.jsonl");
            write_rows(&session, rows);
            build_tui_model(Some(tmp.path()), false, None).expect("build model")
        }

        #[test]
        fn title_bar_includes_package_version() {
            let backend = TestBackend::new(80, 20);
            let mut terminal = Terminal::new(backend).expect("terminal");
            let state = TuiState::new(TuiModel {
                sessions: Vec::new(),
                selected_session: 0,
            });

            terminal.draw(|frame| draw(frame, &state)).expect("draw");

            let rendered = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                rendered.contains(&format!("cxv {}", env!("CARGO_PKG_VERSION"))),
                "rendered buffer did not include package version: {rendered:?}"
            );
        }

        #[test]
        fn osc52_copy_sequence_encodes_selected_text() {
            assert_eq!(
                osc52_copy_sequence("function_call"),
                "\x1b]52;c;ZnVuY3Rpb25fY2FsbA==\x07"
            );
        }

        #[test]
        fn draw_renders_session_panels_and_raw_popup() {
            let tmp = TempDir::new().expect("tempdir");
            let model = model_with_session(
                &tmp,
                vec![
                    json!({
                        "timestamp": "2026-04-25T12:00:00Z",
                        "type": "session_meta",
                        "payload": {"id": "render-session", "cwd": "/repo/render"}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:01Z",
                        "type": "event_msg",
                        "payload": {
                            "type": "token_count",
                            "info": {
                                "model_context_window": 258400,
                                "total_token_usage": {"total_tokens": 1250000}
                            }
                        }
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:02Z",
                        "type": "turn_context",
                        "payload": {
                            "turn_id": "render-turn",
                            "summary": "Rendered compaction summary.",
                            "truncation_policy": {"mode": "tokens", "limit": 12000}
                        }
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:03Z",
                        "type": "response_item",
                        "payload": {"type": "message", "role": "user", "content": "inspect this"}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:04Z",
                        "type": "response_item",
                        "payload": {"type": "message", "role": "assistant", "content": "rendered"}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:05Z",
                        "type": "response_item",
                        "payload": {
                            "type": "function_call",
                            "name": "exec_command",
                            "arguments": "{\"cmd\":\"cargo test --test tui\"}"
                        }
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:06Z",
                        "type": "response_item",
                        "payload": {
                            "type": "function_call_output",
                            "call_id": "call-render",
                            "output": "{\"exit_code\":0,\"stdout\":\"ok\"}"
                        }
                    }),
                ],
            );
            let mut state = TuiState::with_options(model, TuiDisplayMode::Verbose, true);
            state.selected_message = 4;

            let rendered = render_state(&state, 160, 48);

            assert!(rendered.contains("render-session"));
            assert!(rendered.contains("messages:6"));
            assert!(rendered.contains("tokens:1.2m"));
            assert!(rendered.contains("Compactions"));
            assert!(rendered.contains("exec_command"));
            assert!(rendered.contains("REQUEST BODY"));
            assert!(rendered.contains("cargo test --test tui"));
        }

        #[test]
        fn draw_renders_empty_filtered_and_no_compaction_states() {
            let empty_state = TuiState::new(TuiModel {
                sessions: Vec::new(),
                selected_session: 0,
            });
            let rendered = render_state(&empty_state, 120, 36);
            assert!(rendered.contains("No Codex sessions found"));
            let mut empty_nav = empty_state.clone();
            empty_nav.jump_next_compaction();
            empty_nav.jump_previous_compaction();
            handle_key(
                &mut empty_nav,
                KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            );
            assert_eq!(empty_nav.selected_message, 0);

            let tmp = TempDir::new().expect("tempdir");
            let model = model_with_session(
                &tmp,
                vec![
                    json!({
                        "timestamp": "2026-04-25T12:00:00Z",
                        "type": "session_meta",
                        "payload": {"id": "plain-session", "cwd": "/repo/plain"}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:01Z",
                        "type": "response_item",
                        "payload": {"type": "message", "role": "assistant", "content": "plain"}
                    }),
                ],
            );
            let state = TuiState::new(model.clone());
            let rendered = render_state(&state, 120, 36);
            assert!(rendered.contains("No compaction events in this session."));
            assert_eq!(
                state.compaction_summary_text(),
                "No Codex context summary events found."
            );

            let mut filtered = TuiState::new(model);
            filtered.set_session_search("project:missing");
            let rendered = render_state(&filtered, 120, 36);
            assert!(rendered.contains("No sessions match"));
            assert!(rendered.contains("tag filter") || rendered.contains("current search"));
        }

        #[test]
        fn raw_popup_text_handles_empty_rows_response_bodies_and_truncation() {
            let empty_state = TuiState::new(TuiModel {
                sessions: Vec::new(),
                selected_session: 0,
            });
            assert_eq!(empty_state.raw_popup_text(), "No message selected.");
            assert_eq!(empty_state.compaction_summary_text(), "No session selected.");

            let tmp = TempDir::new().expect("tempdir");
            let long_arguments = "x".repeat(40_050);
            let model = model_with_session(
                &tmp,
                vec![
                    json!({
                        "timestamp": "2026-04-25T12:00:00Z",
                        "type": "session_meta",
                        "payload": {"id": "raw-session", "cwd": "/repo/raw"}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:01Z",
                        "type": "response_item",
                        "payload": {
                            "type": "function_call",
                            "name": "large_request",
                            "arguments": long_arguments
                        }
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:02Z",
                        "type": "response_item",
                        "payload": {
                            "type": "function_call_output",
                            "call_id": "large-response",
                            "output": "{\"stdout\":\"done\"}"
                        }
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:03Z",
                        "type": "response_item",
                        "payload": {"type": "message", "role": "assistant", "content": "no raw fields"}
                    }),
                ],
            );
            let mut state = TuiState::with_display_mode(model, TuiDisplayMode::Verbose);

            state.selected_message = 0;
            assert!(state.raw_popup_text().contains("(truncated)"));

            state.selected_message = 1;
            let response_text = state.raw_popup_text();
            assert!(response_text.contains("RESPONSE BODY"));
            assert!(response_text.contains("\"stdout\": \"done\""));

            let no_raw_state = TuiState::new(TuiModel {
                sessions: vec![session_row(ParsedSession {
                    metadata: SessionMetadata {
                        path: PathBuf::from("no-raw.jsonl"),
                        session_id: "no-raw".to_string(),
                        cwd: String::new(),
                        started_at: String::new(),
                        cli_version: String::new(),
                        model_provider: String::new(),
                    },
                    messages: vec![ParsedMessage {
                        line_number: 1,
                        timestamp: String::new(),
                        record_type: "message".to_string(),
                        kind: "message".to_string(),
                        role: "assistant".to_string(),
                        content: "plain message".to_string(),
                        request_body: String::new(),
                        response_body: String::new(),
                        raw_payload: String::new(),
                    }],
                    compaction_events: Vec::new(),
                    stats: ConversationStats {
                        line_count: 1,
                        message_count: 1,
                        ..ConversationStats::default()
                    },
                })],
                selected_session: 0,
            });
            assert!(no_raw_state
                .raw_popup_text()
                .contains("No raw request/response body is available"));
        }

        #[test]
        fn detail_and_summary_text_cover_boundary_tokens_and_truncation_variants() {
            let mut parsed = ParsedSession {
                metadata: SessionMetadata {
                    path: PathBuf::from("synthetic.jsonl"),
                    session_id: "synthetic".to_string(),
                    cwd: "/repo/synthetic".to_string(),
                    started_at: "2026-04-25T12:00:00Z".to_string(),
                    cli_version: String::new(),
                    model_provider: String::new(),
                },
                messages: vec![
                    ParsedMessage {
                        line_number: 7,
                        timestamp: "2026-04-25T12:00:07Z".to_string(),
                        record_type: "user".to_string(),
                        kind: String::new(),
                        role: "user".to_string(),
                        content: "selected compaction row".to_string(),
                        request_body: String::new(),
                        response_body: String::new(),
                        raw_payload: String::new(),
                    },
                    ParsedMessage {
                        line_number: 8,
                        timestamp: String::new(),
                        record_type: "message".to_string(),
                        kind: String::new(),
                        role: String::new(),
                        content: String::new(),
                        request_body: String::new(),
                        response_body: String::new(),
                        raw_payload: String::new(),
                    },
                ],
                compaction_events: vec![
                    CompactionEvent {
                        line_number: 7,
                        timestamp: "2026-04-25T12:00:07Z".to_string(),
                        turn_id: String::new(),
                        summary: "Boundary summary body.".to_string(),
                        truncation_mode: String::new(),
                        truncation_limit: Some(9000),
                        token_usage: Some(TokenUsage {
                            total_tokens: 10000,
                            ..TokenUsage::default()
                        }),
                        source: "boundary_summary".to_string(),
                        boundary_line_number: Some(6),
                        trigger: "manual".to_string(),
                    },
                    CompactionEvent {
                        line_number: 8,
                        timestamp: String::new(),
                        turn_id: String::new(),
                        summary: "Mode-only summary body.".to_string(),
                        truncation_mode: "tokens".to_string(),
                        truncation_limit: None,
                        token_usage: None,
                        source: "turn_context".to_string(),
                        boundary_line_number: None,
                        trigger: String::new(),
                    },
                ],
                stats: ConversationStats {
                    line_count: 8,
                    message_count: 2,
                    total_tokens: 10000,
                    model_context_window: 258400,
                    ..ConversationStats::default()
                },
            };
            parsed.stats.last_timestamp = "2026-04-25T12:00:08Z".to_string();
            let session = session_row(parsed);
            let mut state = TuiState::new(TuiModel {
                sessions: vec![session],
                selected_session: 0,
            });

            let detail = state.selected_detail_text();
            assert!(detail.contains("COMPACTION EVENT line 7"));
            assert!(detail.contains("boundary line: 6"));
            assert!(detail.contains("tokens before: 10.0k"));
            assert!(detail.contains("selected compaction row"));

            state.show_summaries = true;
            let summaries = state.selected_detail_text();
            assert!(summaries.contains("truncation: 9000"));
            assert!(summaries.contains("truncation: tokens"));
            assert!(summaries.contains("COMPACTION 2"));

            state.show_summaries = false;
            state.model.sessions[0].parsed.messages[0].content = "x".repeat(20_050);
            assert!(state.selected_detail_text().contains("(truncated)"));

            state.show_summaries = false;
            state.selected_message = 10;
            assert_eq!(state.selected_detail_text(), "No message selected.");
        }

        #[test]
        fn key_and_mouse_handlers_cover_modal_search_detail_and_navigation_edges() {
            let tmp = TempDir::new().expect("tempdir");
            let model = model_with_session(
                &tmp,
                vec![
                    json!({
                        "timestamp": "2026-04-25T12:00:00Z",
                        "type": "session_meta",
                        "payload": {"id": "input-session", "cwd": "/repo/input"}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:01Z",
                        "type": "turn_context",
                        "payload": {"turn_id": "input-turn", "summary": "Input summary."}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:02Z",
                        "type": "response_item",
                        "payload": {"type": "message", "role": "assistant", "content": "after"}
                    }),
                ],
            );
            let mut state = TuiState::with_options(model, TuiDisplayMode::Tidy, true);
            let area = Rect::new(0, 0, 120, 40);

            handle_key(&mut state, KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
            assert!(!state.mouse_capture_enabled());
            handle_key(&mut state, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
            handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
            handle_key(&mut state, KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
            handle_key(
                &mut state,
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            );
            assert_eq!(state.raw_popup_scroll(), 11);
            handle_key(&mut state, KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
            assert_eq!(state.raw_popup_scroll(), 1);
            handle_key(&mut state, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
            assert!(!state.raw_popup_visible());
            handle_key(&mut state, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
            handle_key(&mut state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert!(!state.raw_popup_visible());
            handle_key(&mut state, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
            handle_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert!(!state.raw_popup_visible());

            handle_key(&mut state, KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
            handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
            handle_key(&mut state, KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT));
            assert_eq!(state.session_search(), "I");
            handle_key(
                &mut state,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            );
            assert_eq!(state.session_search(), "");
            handle_key(&mut state, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
            handle_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            );
            assert_eq!(state.session_search(), "");
            handle_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert_eq!(state.focus(), TuiFocus::History);

            handle_key(&mut state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            handle_key(
                &mut state,
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            );
            assert_eq!(state.detail_scroll(), 10);
            handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
            handle_key(&mut state, KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
            assert_eq!(state.detail_scroll(), 0);
            handle_key(&mut state, KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
            assert!(state.mouse_capture_enabled());
            handle_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert_eq!(state.focus(), TuiFocus::History);

            assert!(handle_key(&mut state, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)));
            assert!(!handle_key(&mut state, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
            assert!(!handle_key(
                &mut state,
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)
            ));
            assert!(!handle_key(
                &mut state,
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)
            ));
            assert!(!handle_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('C'), KeyModifiers::NONE)
            ));
            assert!(!handle_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SHIFT)
            ));
            handle_key(&mut state, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
            assert!(!handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
            assert!(state.show_summaries);
            assert!(handle_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));

            state.mouse_capture_enabled = false;
            let before = state.selected_message_line();
            handle_mouse(
                &mut state,
                MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: 42,
                    row: 14,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            assert_eq!(state.selected_message_line(), before);

            state.mouse_capture_enabled = true;
            state.show_raw_popup = true;
            handle_mouse(
                &mut state,
                MouseEvent {
                    kind: MouseEventKind::Moved,
                    column: 42,
                    row: 14,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            handle_mouse(
                &mut state,
                MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: 42,
                    row: 14,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            assert_eq!(state.raw_popup_scroll(), 3);
            handle_mouse(
                &mut state,
                MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: 42,
                    row: 14,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            assert_eq!(state.raw_popup_scroll(), 0);
            handle_mouse(
                &mut state,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Right),
                    column: 42,
                    row: 14,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            assert!(!state.raw_popup_visible());

            handle_mouse(
                &mut state,
                MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: 42,
                    row: 14,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            handle_mouse(
                &mut state,
                MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: 42,
                    row: 14,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            handle_mouse(
                &mut state,
                MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: 42,
                    row: 25,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
        }

        #[test]
        fn private_helpers_cover_shortening_matching_and_numeric_edges() {
            let parsed = ParsedSession {
                metadata: SessionMetadata {
                    path: PathBuf::from("/tmp/fallback-session.jsonl"),
                    session_id: String::new(),
                    cwd: String::new(),
                    started_at: String::new(),
                    cli_version: String::new(),
                    model_provider: String::new(),
                },
                messages: Vec::new(),
                compaction_events: Vec::new(),
                stats: ConversationStats {
                    first_timestamp: "2026-04-25T12:00:00Z".to_string(),
                    line_count: 1,
                    ..ConversationStats::default()
                },
            };
            let session = session_row(parsed);

            assert_eq!(session.session_id, "fallback-session");
            assert_eq!(session.started_at, "2026-04-25T12:00:00Z");
            assert!(session_matches_term(&session, ""));
            assert!(session_matches_term(&session, "id:fallback"));
            assert!(session_matches_term(&session, "fallback"));
            assert!(!session_matches_term(&session, "unknown:fallback"));
            assert!(!session_matches_term(&session, "tag:unknown"));
            assert_eq!(short("abcdef", 2), "ab");
            assert_eq!(short("abcdef", 5), "ab...");
            assert_eq!(short_time("short"), "short");
            assert_eq!(compact_number(1_250), "1.2k");
            assert_eq!(compact_number(1_250_000), "1.2m");
            assert_eq!(empty_dash(""), "-");

            let no_kind = ParsedMessage {
                line_number: 1,
                timestamp: String::new(),
                record_type: String::new(),
                kind: String::new(),
                role: String::new(),
                content: String::new(),
                request_body: String::new(),
                response_body: String::new(),
                raw_payload: String::new(),
            };
            assert_eq!(display_kind(&no_kind), "message");
        }
    }
}
