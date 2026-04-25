import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from codex_compaction_viewer.parser import discover_sessions, parse_jsonl


def write_jsonl(path: Path, rows: list[dict | str]) -> None:
    with path.open("w", encoding="utf-8") as handle:
        for row in rows:
            if isinstance(row, str):
                handle.write(row + "\n")
            else:
                handle.write(json.dumps(row) + "\n")


class ParserTests(unittest.TestCase):
    def test_parse_jsonl_extracts_codex_context_summary_event(self) -> None:
        with TemporaryDirectory() as tmp:
            session = Path(tmp) / "rollout-2026-04-25T12-00-00-example.jsonl"
            write_jsonl(
                session,
                [
                    {
                        "timestamp": "2026-04-25T12:00:00Z",
                        "type": "session_meta",
                        "payload": {
                            "id": "sess-1",
                            "cwd": "/repo",
                            "cli_version": "0.1.0",
                            "model_provider": "openai",
                        },
                    },
                    "not json",
                    {
                        "timestamp": "2026-04-25T12:01:00Z",
                        "type": "turn_context",
                        "payload": {
                            "turn_id": "turn-1",
                            "model": "gpt-test",
                            "summary": "Prior work has been compacted into this summary.",
                            "truncation_policy": {"mode": "tokens", "limit": 10000},
                        },
                    },
                    {
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
                                    "total_tokens": 1540,
                                },
                                "last_token_usage": {
                                    "input_tokens": 700,
                                    "cached_input_tokens": 100,
                                    "output_tokens": 150,
                                    "reasoning_output_tokens": 20,
                                    "total_tokens": 870,
                                },
                            },
                        },
                    },
                    {
                        "timestamp": "2026-04-25T12:01:07Z",
                        "type": "response_item",
                        "payload": {
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": "Done"}],
                        },
                    },
                ],
            )

            parsed = parse_jsonl(session)

        self.assertEqual(parsed.metadata.session_id, "sess-1")
        self.assertEqual(parsed.metadata.cwd, "/repo")
        self.assertEqual(parsed.stats.bad_lines, 1)
        self.assertEqual(parsed.stats.line_count, 5)
        self.assertEqual(parsed.stats.total_tokens, 1540)
        self.assertEqual(parsed.stats.model_context_window, 258400)
        self.assertEqual(len(parsed.compaction_events), 1)
        event = parsed.compaction_events[0]
        self.assertEqual(event.turn_id, "turn-1")
        self.assertEqual(event.summary_length, 48)
        self.assertEqual(event.truncation_mode, "tokens")
        self.assertEqual(event.truncation_limit, 10000)
        self.assertIn("Prior work", event.summary)
        self.assertGreaterEqual(len(parsed.messages), 3)

    def test_discover_sessions_finds_active_and_archived_sessions(self) -> None:
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            active = root / "sessions" / "2026" / "04" / "25"
            archived = root / "archived_sessions"
            active.mkdir(parents=True)
            archived.mkdir(parents=True)
            active_file = active / "rollout-active.jsonl"
            archived_file = archived / "rollout-archived.jsonl"
            active_file.write_text("{}", encoding="utf-8")
            archived_file.write_text("{}", encoding="utf-8")

            active_only = discover_sessions(root, include_archived=False)
            with_archived = discover_sessions(root, include_archived=True)

        self.assertEqual(active_only, [active_file])
        self.assertEqual(with_archived, [archived_file, active_file])

    def test_parse_jsonl_ignores_legacy_auto_summary_mode(self) -> None:
        with TemporaryDirectory() as tmp:
            session = Path(tmp) / "legacy.jsonl"
            write_jsonl(
                session,
                [
                    {
                        "timestamp": "2026-04-25T12:00:00Z",
                        "type": "turn_context",
                        "payload": {
                            "turn_id": "legacy-turn",
                            "summary": "auto",
                            "truncation_policy": {"mode": "tokens", "limit": 10000},
                        },
                    }
                ],
            )

            parsed = parse_jsonl(session)

        self.assertEqual(parsed.compaction_events, [])


if __name__ == "__main__":
    unittest.main()
