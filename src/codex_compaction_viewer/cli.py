from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
from typing import TextIO

from .parser import CompactionEvent, ParsedSession, discover_sessions, parse_jsonl


def main(argv: list[str] | None = None, out: TextIO | None = None, err: TextIO | None = None) -> int:
    output = out or sys.stdout
    errors = err or sys.stderr
    parser = _arg_parser()
    args = parser.parse_args(argv)

    try:
        if args.summary:
            parsed = parse_jsonl(args.summary)
            if args.json:
                print(json.dumps(_session_dict(parsed), ensure_ascii=False, indent=2), file=output)
            else:
                _print_summary(parsed, output)
            return 0

        if args.file:
            parsed = parse_jsonl(args.file)
            if args.json:
                print(json.dumps(_session_dict(parsed), ensure_ascii=False, indent=2), file=output)
            else:
                _print_session(parsed, output)
            return 0

        if args.scan:
            sessions = [parse_jsonl(path) for path in discover_sessions(args.root, args.include_archived)]
            rows = [_scan_row(session) for session in sessions]
            if args.json:
                print(json.dumps(rows, ensure_ascii=False, indent=2), file=output)
            else:
                _print_scan(rows, output)
            return 0

        parser.print_help(output)
        return 0
    except OSError as exc:
        print(f"cxv: {exc}", file=errors)
        return 1


def _arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="cxv",
        description="Inspect Codex JSONL sessions for context summaries, token usage, and truncation signals.",
    )
    parser.add_argument("file", nargs="?", type=Path, help="Codex JSONL file to inspect")
    parser.add_argument("--scan", action="store_true", help="scan ~/.codex/sessions for JSONL files")
    parser.add_argument("--root", type=Path, default=None, help="Codex home root, defaults to ~/.codex")
    parser.add_argument("--include-archived", action="store_true", help="include ~/.codex/archived_sessions")
    parser.add_argument("--summary", type=Path, help="print context summary events for a JSONL session")
    parser.add_argument("--json", action="store_true", help="emit structured JSON")
    return parser


def _scan_row(session: ParsedSession) -> dict[str, object]:
    stats = session.stats
    metadata = session.metadata
    return {
        "path": str(metadata.path),
        "session_id": metadata.session_id or metadata.path.stem,
        "cwd": metadata.cwd,
        "started_at": metadata.started_at or stats.first_timestamp,
        "last_timestamp": stats.last_timestamp,
        "lines": stats.line_count,
        "bad_lines": stats.bad_lines,
        "messages": stats.message_count,
        "compactions": len(session.compaction_events),
        "token_count_events": stats.token_count_events,
        "total_tokens": stats.total_tokens,
        "input_tokens": stats.input_tokens,
        "cached_input_tokens": stats.cached_input_tokens,
        "output_tokens": stats.output_tokens,
        "reasoning_output_tokens": stats.reasoning_output_tokens,
        "model_context_window": stats.model_context_window,
    }


def _session_dict(session: ParsedSession) -> dict[str, object]:
    row = _scan_row(session)
    row["compaction_events"] = [_event_dict(event) for event in session.compaction_events]
    return row


def _event_dict(event: CompactionEvent) -> dict[str, object]:
    token_usage = event.token_usage
    return {
        "line": event.line_number,
        "timestamp": event.timestamp,
        "turn_id": event.turn_id,
        "summary_length": event.summary_length,
        "summary": event.summary,
        "truncation_mode": event.truncation_mode,
        "truncation_limit": event.truncation_limit,
        "tokens_before": token_usage.total_tokens if token_usage else 0,
    }


def _print_scan(rows: list[dict[str, object]], out: TextIO) -> None:
    if not rows:
        print("No Codex sessions found.", file=out)
        return

    headers = ["Session", "Compactions", "Lines", "Tokens", "Context", "CWD"]
    table_rows = [
        [
            _short(str(row["session_id"]), 18),
            str(row["compactions"]),
            str(row["lines"]),
            _compact_number(int(row["total_tokens"])),
            _compact_number(int(row["model_context_window"])),
            _short(str(row["cwd"]), 44),
        ]
        for row in rows
    ]
    _print_table(headers, table_rows, out)


def _print_session(session: ParsedSession, out: TextIO) -> None:
    row = _scan_row(session)
    print(f"Session: {row['session_id']}", file=out)
    print(f"Path: {row['path']}", file=out)
    if row["cwd"]:
        print(f"CWD: {row['cwd']}", file=out)
    print(
        f"Lines: {row['lines']}  Messages: {row['messages']}  "
        f"Compactions: {row['compactions']}  Tokens: {_compact_number(int(row['total_tokens']))}",
        file=out,
    )
    print("", file=out)
    _print_summary(session, out)


def _print_summary(session: ParsedSession, out: TextIO) -> None:
    if not session.compaction_events:
        print("No Codex context summary events found.", file=out)
        return

    for index, event in enumerate(session.compaction_events, start=1):
        policy = event.truncation_mode
        if event.truncation_limit is not None:
            policy = f"{policy}:{event.truncation_limit}" if policy else str(event.truncation_limit)
        tokens = event.token_usage.total_tokens if event.token_usage else 0
        print(f"#{index} line {event.line_number} turn {event.turn_id}", file=out)
        if event.timestamp:
            print(f"timestamp: {event.timestamp}", file=out)
        if policy:
            print(f"truncation: {policy}", file=out)
        if tokens:
            print(f"tokens before: {_compact_number(tokens)}", file=out)
        print(event.summary, file=out)
        if index != len(session.compaction_events):
            print("", file=out)


def _print_table(headers: list[str], rows: list[list[str]], out: TextIO) -> None:
    widths = [
        max(len(headers[column]), *(len(row[column]) for row in rows))
        for column in range(len(headers))
    ]
    print("  ".join(header.ljust(widths[index]) for index, header in enumerate(headers)), file=out)
    print("  ".join("-" * width for width in widths), file=out)
    for row in rows:
        print("  ".join(value.ljust(widths[index]) for index, value in enumerate(row)), file=out)


def _short(value: str, limit: int) -> str:
    if len(value) <= limit:
        return value
    if limit <= 1:
        return value[:limit]
    return value[: limit - 1] + "…"


def _compact_number(value: int) -> str:
    if value >= 1_000_000:
        return f"{value / 1_000_000:.1f}m"
    if value >= 1_000:
        return f"{value / 1_000:.1f}k"
    return str(value)


if __name__ == "__main__":
    raise SystemExit(main())

