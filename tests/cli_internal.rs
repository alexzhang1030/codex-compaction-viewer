pub use codex_compaction_viewer::{parser, tui, version_line, APP_NAME, APP_VERSION};

mod cli_internal {
    include!("../src/cli.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;
        use std::fs;
        use std::io::Write;
        use std::path::Path;
        use tempfile::TempDir;

        fn write_session(path: &Path, rows: &[serde_json::Value]) {
            fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
            let mut file = fs::File::create(path).expect("fixture");
            for row in rows {
                writeln!(file, "{row}").expect("write row");
            }
        }

        #[test]
        fn run_from_covers_help_tui_and_scan_modes() {
            let help = run_from(["cxv"]).expect("help");
            assert!(help.contains("Inspect Codex JSONL sessions"));

            let tui = run_from(["cxv", "--tui"]).expect("tui");
            assert!(tui.contains("Interactive TUI requires a terminal"));

            let tmp = TempDir::new().expect("tempdir");
            let empty_scan = run_from(["cxv", "--scan", "--root", tmp.path().to_str().unwrap()])
                .expect("empty scan");
            assert_eq!(empty_scan, "No Codex sessions found.\n");

            let session = tmp.path().join("sessions/2026/04/25/scan.jsonl");
            write_session(
                &session,
                &[
                    json!({
                        "timestamp": "2026-04-25T12:00:00Z",
                        "type": "event_msg",
                        "payload": {
                            "type": "token_count",
                            "info": {
                                "model_context_window": 258400,
                                "total_token_usage": {"total_tokens": 4200}
                            }
                        }
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:01:00Z",
                        "type": "turn_context",
                        "payload": {
                            "turn_id": "turn-scan",
                            "summary": "Scan summary"
                        }
                    }),
                ],
            );

            let scan = run_from(["cxv", "--scan", "--root", tmp.path().to_str().unwrap()])
                .expect("scan");
            assert!(scan.contains("Session"));
            assert!(scan.contains("4.2k"));
            assert!(scan.contains("258.4k"));
        }

        #[test]
        fn run_from_covers_file_and_summary_output_modes() {
            let tmp = TempDir::new().expect("tempdir");
            let session = tmp.path().join("fixture.jsonl");
            write_session(
                &session,
                &[
                    json!({
                        "timestamp": "2026-04-25T12:00:00Z",
                        "type": "session_meta",
                        "payload": {"cwd": "/workspace/demo"}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:01Z",
                        "type": "event_msg",
                        "payload": {
                            "type": "token_count",
                            "info": {
                                "model_context_window": 8192,
                                "total_token_usage": {"total_tokens": 999}
                            }
                        }
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:00:02Z",
                        "type": "turn_context",
                        "payload": {
                            "turn_id": "turn-file",
                            "summary": "File summary",
                            "truncation_policy": {"mode": "tokens", "limit": 12000}
                        }
                    }),
                ],
            );

            let session_text = run_from(["cxv", session.to_str().unwrap()]).expect("session text");
            assert!(session_text.contains("Session: fixture"));
            assert!(session_text.contains("Path:"));
            assert!(session_text.contains("File summary"));

            let session_json =
                run_from(["cxv", session.to_str().unwrap(), "--json"]).expect("session json");
            let output: serde_json::Value = serde_json::from_str(&session_json).expect("json");
            assert_eq!(output["session_id"], "fixture");
            assert_eq!(output["compaction_events"][0]["tokens_before"], 999);

            let summary_json = run_from(["cxv", "--summary", session.to_str().unwrap(), "--json"])
                .expect("summary");
            let summary_output: serde_json::Value =
                serde_json::from_str(&summary_json).expect("summary json");
            assert_eq!(summary_output["cwd"], "/workspace/demo");
            assert_eq!(summary_output["compaction_events"][0]["turn_id"], "turn-file");
        }

        #[test]
        fn formatting_helpers_cover_truncation_and_padding() {
            assert_eq!(short("abc", 3), "abc");
            assert_eq!(short("abcdef", 3), "abc");
            assert_eq!(short("abcdef", 5), "ab...");
            assert_eq!(compact_number(999), "999");
            assert_eq!(compact_number(12_345), "12.3k");
            assert_eq!(compact_number(2_500_000), "2.5m");

            let table =
                print_table(&["Name", "Value"], &[vec!["x".to_string(), "1".to_string()]]);
            assert!(table.contains("Name"));
            assert!(table.contains("Value"));
            assert!(table.contains("x"));
            assert_eq!(
                join_padded(["a".to_string(), "bb".to_string()].into_iter(), &[2, 3]),
                "a   bb "
            );
        }

        #[test]
        fn display_mode_arg_maps_to_tui_modes() {
            assert!(matches!(
                TuiDisplayMode::from(DisplayModeArg::Tidy),
                TuiDisplayMode::Tidy
            ));
            assert!(matches!(
                TuiDisplayMode::from(DisplayModeArg::Verbose),
                TuiDisplayMode::Verbose
            ));
        }

        #[test]
        fn should_launch_tui_requires_non_versioned_tty_invocation() {
            let default_args = Args {
                file: None,
                scan: false,
                root: None,
                include_archived: false,
                summary: None,
                json: false,
                tui: false,
                mode: DisplayModeArg::Tidy,
                raw_bodies: false,
                no_mouse: false,
                version: false,
            };

            assert!(should_launch_tui_with(&default_args, 1, true, true));
            assert!(!should_launch_tui_with(&default_args, 2, true, true));
            assert!(!should_launch_tui_with(&default_args, 1, false, true));
            assert!(!should_launch_tui_with(&default_args, 1, true, false));

            let explicit_tui = Args {
                tui: true,
                ..default_args
            };
            assert!(should_launch_tui_with(&explicit_tui, 3, true, true));

            let version_args = Args {
                version: true,
                tui: true,
                ..explicit_tui
            };
            assert!(!should_launch_tui_with(&version_args, 3, true, true));
        }

        #[test]
        fn mouse_capture_tracks_no_mouse_flag() {
            let default_args = Args {
                file: None,
                scan: false,
                root: None,
                include_archived: false,
                summary: None,
                json: false,
                tui: false,
                mode: DisplayModeArg::Tidy,
                raw_bodies: false,
                no_mouse: false,
                version: false,
            };

            assert!(mouse_capture_enabled(&default_args));

            let no_mouse_args = Args {
                no_mouse: true,
                ..default_args
            };
            assert!(!mouse_capture_enabled(&no_mouse_args));
        }

        #[test]
        fn print_session_omits_empty_cwd_and_print_summary_formats_optional_fields() {
            let tmp = TempDir::new().expect("tempdir");
            let session = tmp.path().join("format.jsonl");
            write_session(
                &session,
                &[
                    json!({
                        "timestamp": "",
                        "type": "session_meta",
                        "payload": {"id": "format-session", "cwd": ""}
                    }),
                    json!({
                        "timestamp": "2026-04-25T12:01:00Z",
                        "type": "turn_context",
                        "payload": {
                            "turn_id": "",
                            "summary": "First summary",
                            "truncation_policy": {"limit": 9000}
                        }
                    }),
                    json!({
                        "timestamp": "",
                        "type": "turn_context",
                        "payload": {
                            "turn_id": "turn-two",
                            "summary": "Second summary",
                            "truncation_policy": {"mode": "tokens"}
                        }
                    }),
                ],
            );

            let parsed = parse_jsonl(&session).expect("parse");
            let session_text = print_session(&parsed);
            assert!(session_text.contains("Session: format-session"));
            assert!(session_text.contains("Path:"));
            assert!(!session_text.contains("CWD:"));

            let summary = print_summary(&parsed);
            assert!(summary.starts_with(
                "#1 line 2\ntimestamp: 2026-04-25T12:01:00Z\ntruncation: 9000\n"
            ));
            assert!(summary.contains("\n#2 line 3 turn turn-two\n"));
            assert!(summary.contains("truncation: tokens\n"));
            assert!(!summary.contains("timestamp: \n"));
            assert!(summary.contains("First summary\n\n#2 line 3 turn turn-two\n"));
        }
    }
}
