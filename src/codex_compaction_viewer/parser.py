from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
import json
from pathlib import Path
from typing import Any, Iterable


EMPTY_SUMMARY_VALUES = {"", "auto", "manual", "null", "none", "nil"}


@dataclass(slots=True)
class SessionMetadata:
    path: Path
    session_id: str = ""
    cwd: str = ""
    started_at: str = ""
    cli_version: str = ""
    model_provider: str = ""


@dataclass(slots=True)
class ParsedMessage:
    line_number: int
    timestamp: str
    record_type: str
    kind: str
    role: str = ""
    content: str = ""


@dataclass(slots=True)
class TokenUsage:
    input_tokens: int = 0
    cached_input_tokens: int = 0
    output_tokens: int = 0
    reasoning_output_tokens: int = 0
    total_tokens: int = 0


@dataclass(slots=True)
class CompactionEvent:
    line_number: int
    timestamp: str
    turn_id: str
    summary: str
    truncation_mode: str = ""
    truncation_limit: int | None = None
    token_usage: TokenUsage | None = None
    source: str = "turn_context"
    boundary_line_number: int | None = None
    trigger: str = ""

    @property
    def summary_length(self) -> int:
        return len(self.summary)


@dataclass(slots=True)
class ConversationStats:
    line_count: int = 0
    bad_lines: int = 0
    message_count: int = 0
    token_count_events: int = 0
    input_tokens: int = 0
    cached_input_tokens: int = 0
    output_tokens: int = 0
    reasoning_output_tokens: int = 0
    total_tokens: int = 0
    model_context_window: int = 0
    first_timestamp: str = ""
    last_timestamp: str = ""


@dataclass(slots=True)
class ParsedSession:
    metadata: SessionMetadata
    messages: list[ParsedMessage] = field(default_factory=list)
    compaction_events: list[CompactionEvent] = field(default_factory=list)
    stats: ConversationStats = field(default_factory=ConversationStats)


@dataclass(slots=True)
class PendingBoundary:
    line_number: int
    timestamp: str
    trigger: str = ""
    token_usage: TokenUsage | None = None


def discover_sessions(root: Path | str | None = None, include_archived: bool = False) -> list[Path]:
    base = Path(root).expanduser() if root else Path.home() / ".codex"
    patterns = [base / "sessions"]
    if include_archived:
        patterns.append(base / "archived_sessions")

    files: list[Path] = []
    for directory in patterns:
        if directory.exists():
            files.extend(directory.rglob("*.jsonl"))
    return sorted(files)


def parse_jsonl(path: Path | str) -> ParsedSession:
    session_path = Path(path).expanduser()
    parsed = ParsedSession(metadata=SessionMetadata(path=session_path))
    latest_token_usage: TokenUsage | None = None
    pending_boundary: PendingBoundary | None = None

    with session_path.open("r", encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            parsed.stats.line_count += 1
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                parsed.stats.bad_lines += 1
                continue

            if not isinstance(record, dict):
                parsed.stats.bad_lines += 1
                continue

            timestamp = _string(record.get("timestamp"))
            if timestamp:
                _update_time_bounds(parsed.stats, timestamp)

            record_type = _string(record.get("type"))
            payload = record.get("payload")
            if not isinstance(payload, dict):
                payload = {}

            if record_type == "session_meta":
                pending_boundary = None
                _apply_session_meta(parsed.metadata, payload, timestamp)
            elif record_type == "turn_context":
                pending_boundary = None
                event = _parse_turn_context(line_number, timestamp, payload, latest_token_usage)
                if event:
                    parsed.compaction_events.append(event)
                parsed.messages.append(_message_from_turn_context(line_number, timestamp, payload))
            elif record_type == "event_msg":
                pending_boundary = None
                message = _message_from_event(line_number, timestamp, payload)
                parsed.messages.append(message)
                if payload.get("type") == "token_count":
                    parsed.stats.token_count_events += 1
                    latest_token_usage = _apply_token_count(parsed.stats, payload)
            elif record_type == "response_item":
                pending_boundary = None
                parsed.messages.append(_message_from_response_item(line_number, timestamp, payload))
            elif not payload and (boundary := _parse_raw_boundary(line_number, timestamp, record)):
                pending_boundary = boundary
                parsed.messages.append(_message_from_raw_record(line_number, timestamp, record))
            elif not payload and _is_raw_compact_summary(record):
                event = _parse_raw_compact_summary(line_number, timestamp, record, pending_boundary)
                if event:
                    parsed.compaction_events.append(event)
                pending_boundary = None
                parsed.messages.append(_message_from_raw_record(line_number, timestamp, record))
            elif not payload and record_type in {"system", "user", "assistant"}:
                pending_boundary = None
                parsed.messages.append(_message_from_raw_record(line_number, timestamp, record))
            else:
                pending_boundary = None
                parsed.messages.append(
                    ParsedMessage(
                        line_number=line_number,
                        timestamp=timestamp,
                        record_type=record_type,
                        kind=_string(payload.get("type")) or record_type,
                    )
                )

    parsed.stats.message_count = len(parsed.messages)
    return parsed


def parse_many(paths: Iterable[Path | str]) -> list[ParsedSession]:
    return [parse_jsonl(path) for path in paths]


def _apply_session_meta(metadata: SessionMetadata, payload: dict[str, Any], timestamp: str) -> None:
    metadata.session_id = _string(payload.get("id")) or metadata.session_id
    metadata.cwd = _string(payload.get("cwd")) or metadata.cwd
    metadata.cli_version = _string(payload.get("cli_version")) or metadata.cli_version
    metadata.model_provider = _string(payload.get("model_provider")) or metadata.model_provider
    metadata.started_at = timestamp or _string(payload.get("timestamp")) or metadata.started_at


def _parse_turn_context(
    line_number: int,
    timestamp: str,
    payload: dict[str, Any],
    latest_token_usage: TokenUsage | None,
) -> CompactionEvent | None:
    summary = _summary_text(payload.get("summary"))
    if not summary:
        return None

    policy = payload.get("truncation_policy")
    if not isinstance(policy, dict):
        policy = {}

    return CompactionEvent(
        line_number=line_number,
        timestamp=timestamp,
        turn_id=_string(payload.get("turn_id")),
        summary=summary,
        truncation_mode=_string(policy.get("mode")),
        truncation_limit=_int_or_none(policy.get("limit")),
        token_usage=latest_token_usage,
    )


def _parse_raw_boundary(
    line_number: int,
    timestamp: str,
    record: dict[str, Any],
) -> PendingBoundary | None:
    if _string(record.get("type")) != "system" or _string(record.get("subtype")) != "compact_boundary":
        return None

    metadata = record.get("compactMetadata")
    if not isinstance(metadata, dict):
        metadata = record.get("compact_metadata")
    if not isinstance(metadata, dict):
        metadata = {}

    token_count = _first_int(
        metadata,
        (
            "preCompactTokens",
            "pre_compact_tokens",
            "tokensBefore",
            "tokens_before",
            "totalTokens",
            "total_tokens",
        ),
    )
    token_usage = TokenUsage(total_tokens=token_count) if token_count is not None else None

    return PendingBoundary(
        line_number=line_number,
        timestamp=timestamp,
        trigger=_string(metadata.get("trigger")) or _string(record.get("trigger")),
        token_usage=token_usage,
    )


def _parse_raw_compact_summary(
    line_number: int,
    timestamp: str,
    record: dict[str, Any],
    boundary: PendingBoundary | None,
) -> CompactionEvent | None:
    summary = (
        _summary_text(record.get("summary"))
        or _summary_text(record.get("compactSummary"))
        or _summary_text(record.get("compact_summary"))
        or _summary_text(_message_text(record.get("message")))
        or _summary_text(record.get("content"))
    )
    if not summary:
        return None

    return CompactionEvent(
        line_number=line_number,
        timestamp=timestamp,
        turn_id=_string(record.get("uuid")) or _string(record.get("id")),
        summary=summary,
        token_usage=boundary.token_usage if boundary else None,
        source="boundary_summary" if boundary else "compact_summary",
        boundary_line_number=boundary.line_number if boundary else None,
        trigger=(boundary.trigger if boundary else "") or _string(record.get("trigger")),
    )


def _is_raw_compact_summary(record: dict[str, Any]) -> bool:
    return _string(record.get("type")) == "user" and bool(record.get("isCompactSummary"))


def _message_from_turn_context(line_number: int, timestamp: str, payload: dict[str, Any]) -> ParsedMessage:
    policy = payload.get("truncation_policy")
    policy_text = ""
    if isinstance(policy, dict):
        mode = _string(policy.get("mode"))
        limit = _string(policy.get("limit"))
        policy_text = f"{mode}:{limit}" if mode or limit else ""

    return ParsedMessage(
        line_number=line_number,
        timestamp=timestamp,
        record_type="turn_context",
        kind="turn_context",
        role="system",
        content=policy_text,
    )


def _message_from_event(line_number: int, timestamp: str, payload: dict[str, Any]) -> ParsedMessage:
    kind = _string(payload.get("type"))
    if kind == "user_message":
        role = "user"
        content = _string(payload.get("message"))
    elif kind == "agent_message":
        role = "assistant"
        content = _string(payload.get("message"))
    elif kind == "exec_command_end":
        role = "tool"
        status = _string(payload.get("status")) or _string(payload.get("exit_code"))
        command = payload.get("command")
        if isinstance(command, list):
            command_text = " ".join(_string(part) for part in command)
        else:
            command_text = _string(command)
        content = f"{command_text} {status}".strip()
    elif kind == "token_count":
        role = "system"
        info = payload.get("info")
        total_tokens = ""
        if isinstance(info, dict):
            total = info.get("total_token_usage")
            if isinstance(total, dict):
                total_tokens = _string(total.get("total_tokens"))
        content = f"tokens={total_tokens}" if total_tokens else ""
    else:
        role = "system"
        content = _string(payload.get("message")) or _string(payload.get("text"))

    return ParsedMessage(
        line_number=line_number,
        timestamp=timestamp,
        record_type="event_msg",
        kind=kind,
        role=role,
        content=content,
    )


def _message_from_response_item(line_number: int, timestamp: str, payload: dict[str, Any]) -> ParsedMessage:
    kind = _string(payload.get("type"))
    role = _string(payload.get("role"))
    if kind == "function_call":
        role = "tool_call"
        content = _string(payload.get("name"))
    elif kind == "function_call_output":
        role = "tool"
        content = _string(payload.get("call_id"))
    else:
        content = _content_text(payload.get("content"))

    return ParsedMessage(
        line_number=line_number,
        timestamp=timestamp,
        record_type="response_item",
        kind=kind,
        role=role,
        content=content,
    )


def _message_from_raw_record(line_number: int, timestamp: str, record: dict[str, Any]) -> ParsedMessage:
    record_type = _string(record.get("type"))
    subtype = _string(record.get("subtype"))
    content = _message_text(record.get("message")) or _summary_text(record.get("content"))
    if subtype == "compact_boundary":
        metadata = record.get("compactMetadata")
        if not isinstance(metadata, dict):
            metadata = {}
        content = f"compact boundary {metadata.get('trigger', '')}".strip()

    return ParsedMessage(
        line_number=line_number,
        timestamp=timestamp,
        record_type=record_type,
        kind=subtype or record_type,
        role=record_type,
        content=content,
    )


def _apply_token_count(stats: ConversationStats, payload: dict[str, Any]) -> TokenUsage | None:
    info = payload.get("info")
    if not isinstance(info, dict):
        return None

    window = _int_or_none(info.get("model_context_window"))
    if window is not None:
        stats.model_context_window = window

    usage = _token_usage(info.get("total_token_usage"))
    if usage is None:
        return None

    stats.input_tokens = usage.input_tokens
    stats.cached_input_tokens = usage.cached_input_tokens
    stats.output_tokens = usage.output_tokens
    stats.reasoning_output_tokens = usage.reasoning_output_tokens
    stats.total_tokens = usage.total_tokens
    return usage


def _token_usage(value: Any) -> TokenUsage | None:
    if not isinstance(value, dict):
        return None
    return TokenUsage(
        input_tokens=_int(value.get("input_tokens")),
        cached_input_tokens=_int(value.get("cached_input_tokens")),
        output_tokens=_int(value.get("output_tokens")),
        reasoning_output_tokens=_int(value.get("reasoning_output_tokens")),
        total_tokens=_int(value.get("total_tokens")),
    )


def _content_text(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts: list[str] = []
        for item in content:
            if isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict):
                parts.append(_string(item.get("text")) or _string(item.get("summary")))
        return "\n".join(part for part in parts if part)
    return ""


def _message_text(message: Any) -> str:
    if isinstance(message, dict):
        return (
            _content_text(message.get("content"))
            or _string(message.get("text"))
            or _string(message.get("summary"))
        )
    return _content_text(message)


def _summary_text(value: Any) -> str:
    if isinstance(value, str):
        summary = value.strip()
    elif isinstance(value, list):
        summary = _content_text(value).strip()
    elif isinstance(value, dict):
        summary = _string(value.get("text")) or _string(value.get("summary"))
        summary = summary.strip()
    else:
        summary = ""

    if summary.lower() in EMPTY_SUMMARY_VALUES:
        return ""
    return summary


def _update_time_bounds(stats: ConversationStats, timestamp: str) -> None:
    if not stats.first_timestamp or _timestamp_key(timestamp) < _timestamp_key(stats.first_timestamp):
        stats.first_timestamp = timestamp
    if not stats.last_timestamp or _timestamp_key(timestamp) > _timestamp_key(stats.last_timestamp):
        stats.last_timestamp = timestamp


def _timestamp_key(timestamp: str) -> datetime | str:
    try:
        return datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
    except ValueError:
        return timestamp


def _string(value: Any) -> str:
    if value is None:
        return ""
    return str(value)


def _int(value: Any) -> int:
    parsed = _int_or_none(value)
    return parsed if parsed is not None else 0


def _int_or_none(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _first_int(values: dict[str, Any], keys: tuple[str, ...]) -> int | None:
    for key in keys:
        if key in values:
            parsed = _int_or_none(values.get(key))
            if parsed is not None:
                return parsed
    return None
