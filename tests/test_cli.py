import io
import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from codex_compaction_viewer.cli import main


def write_session(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    rows = [
        {
            "timestamp": "2026-04-25T12:00:00Z",
            "type": "session_meta",
            "payload": {"id": "sess-cli", "cwd": "/workspace/demo"},
        },
        {
            "timestamp": "2026-04-25T12:01:00Z",
            "type": "turn_context",
            "payload": {
                "turn_id": "turn-cli",
                "summary": "A compacted context snapshot for CLI tests.",
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
                    "total_token_usage": {"total_tokens": 42},
                },
            },
        },
    ]
    path.write_text("\n".join(json.dumps(row) for row in rows), encoding="utf-8")


class CliTests(unittest.TestCase):
    def test_scan_json_outputs_structured_session_rows(self) -> None:
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            session = root / "sessions" / "2026" / "04" / "25" / "rollout-cli.jsonl"
            write_session(session)
            out = io.StringIO()

            code = main(["--scan", "--root", str(root), "--json"], out=out)

        self.assertEqual(code, 0)
        rows = json.loads(out.getvalue())
        self.assertEqual(rows[0]["session_id"], "sess-cli")
        self.assertEqual(rows[0]["compactions"], 1)
        self.assertEqual(rows[0]["total_tokens"], 42)
        self.assertEqual(rows[0]["cwd"], "/workspace/demo")

    def test_summary_outputs_context_summary_details(self) -> None:
        with TemporaryDirectory() as tmp:
            session = Path(tmp) / "rollout-cli.jsonl"
            write_session(session)
            out = io.StringIO()

            code = main(["--summary", str(session)], out=out)

        text = out.getvalue()
        self.assertEqual(code, 0)
        self.assertIn("turn-cli", text)
        self.assertIn("A compacted context snapshot", text)
        self.assertIn("tokens:10000", text)


if __name__ == "__main__":
    unittest.main()

