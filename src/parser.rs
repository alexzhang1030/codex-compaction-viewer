use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const EMPTY_SUMMARY_VALUES: &[&str] = &["", "auto", "manual", "null", "none", "nil"];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SessionMetadata {
    pub path: PathBuf,
    pub session_id: String,
    pub cwd: String,
    pub started_at: String,
    pub cli_version: String,
    pub model_provider: String,
}

impl SessionMetadata {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            session_id: String::new(),
            cwd: String::new(),
            started_at: String::new(),
            cli_version: String::new(),
            model_provider: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParsedMessage {
    pub line_number: usize,
    pub timestamp: String,
    pub record_type: String,
    pub kind: String,
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub request_body: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub response_body: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub raw_payload: String,
}

impl ParsedMessage {
    fn new(
        line_number: usize,
        timestamp: &str,
        record_type: impl Into<String>,
        kind: impl Into<String>,
        role: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            line_number,
            timestamp: timestamp.to_string(),
            record_type: record_type.into(),
            kind: kind.into(),
            role: role.into(),
            content: content.into(),
            request_body: String::new(),
            response_body: String::new(),
            raw_payload: String::new(),
        }
    }

    fn with_raw_payload(mut self, payload: &Map<String, Value>) -> Self {
        self.raw_payload = format_object_json(payload);
        self
    }

    fn with_request_body(mut self, body: String) -> Self {
        self.request_body = body;
        self
    }

    fn with_response_body(mut self, body: String) -> Self {
        self.response_body = body;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CompactionEvent {
    pub line_number: usize,
    pub timestamp: String,
    pub turn_id: String,
    pub summary: String,
    pub truncation_mode: String,
    pub truncation_limit: Option<i64>,
    pub token_usage: Option<TokenUsage>,
    pub source: String,
    pub boundary_line_number: Option<usize>,
    pub trigger: String,
}

impl CompactionEvent {
    pub fn summary_length(&self) -> usize {
        self.summary.chars().count()
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ConversationStats {
    pub line_count: usize,
    pub bad_lines: usize,
    pub message_count: usize,
    pub token_count_events: usize,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
    pub model_context_window: i64,
    pub first_timestamp: String,
    pub last_timestamp: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParsedSession {
    pub metadata: SessionMetadata,
    pub messages: Vec<ParsedMessage>,
    pub compaction_events: Vec<CompactionEvent>,
    pub stats: ConversationStats,
}

impl ParsedSession {
    fn new(path: PathBuf) -> Self {
        Self {
            metadata: SessionMetadata::new(path),
            messages: Vec::new(),
            compaction_events: Vec::new(),
            stats: ConversationStats::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingBoundary {
    line_number: usize,
    timestamp: String,
    trigger: String,
    token_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Default)]
struct LoadedRecords {
    records: Vec<JsonRecord>,
    line_count: usize,
    bad_lines: usize,
}

#[derive(Debug, Clone)]
struct JsonRecord {
    line_number: usize,
    record: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ResponseItemCoverage {
    user_messages: bool,
    assistant_messages: bool,
    reasoning: bool,
}

pub fn discover_sessions(root: Option<&Path>, include_archived: bool) -> Result<Vec<PathBuf>> {
    let base = match root {
        Some(path) => path.to_path_buf(),
        None => env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")?
            .join(".codex"),
    };

    let mut directories = vec![base.join("sessions")];
    if include_archived {
        directories.push(base.join("archived_sessions"));
    }

    let mut files = Vec::new();
    for directory in directories {
        if !directory.exists() {
            continue;
        }
        for entry in WalkDir::new(directory).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file()
                && entry.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            {
                files.push(entry.path().to_path_buf());
            }
        }
    }
    files.sort();
    Ok(files)
}

pub fn parse_many(paths: &[PathBuf]) -> Result<Vec<ParsedSession>> {
    paths.iter().map(parse_jsonl).collect()
}

pub fn parse_jsonl(path: impl AsRef<Path>) -> Result<ParsedSession> {
    let session_path = path.as_ref().to_path_buf();
    let loaded = load_records(&session_path)?;
    let coverage = response_item_coverage(&loaded.records);
    let mut parsed = ParsedSession::new(session_path);
    parsed.stats.line_count = loaded.line_count;
    parsed.stats.bad_lines = loaded.bad_lines;
    let mut latest_token_usage: Option<TokenUsage> = None;
    let mut pending_boundary: Option<PendingBoundary> = None;
    let mut legacy_context_compacted_events: Vec<CompactionEvent> = Vec::new();

    for JsonRecord {
        line_number,
        record,
    } in loaded.records
    {
        let timestamp = string(record.get("timestamp"));
        if !timestamp.is_empty() {
            update_time_bounds(&mut parsed.stats, &timestamp);
        }

        let record_type = string(record.get("type"));
        let payload = record.get("payload").and_then(Value::as_object);
        let payload_empty = payload.map(Map::is_empty).unwrap_or(true);
        let empty_payload = Map::new();
        let payload = payload.unwrap_or(&empty_payload);

        match record_type.as_str() {
            "session_meta" => {
                pending_boundary = None;
                apply_session_meta(&mut parsed.metadata, payload, &timestamp);
            }
            "turn_context" => {
                pending_boundary = None;
                if let Some(event) =
                    parse_turn_context(line_number, &timestamp, payload, latest_token_usage.clone())
                {
                    parsed.compaction_events.push(event);
                }
                parsed
                    .messages
                    .push(message_from_turn_context(line_number, &timestamp, payload));
            }
            "compacted" => {
                pending_boundary = None;
                parsed.compaction_events.push(parse_compacted(
                    line_number,
                    &timestamp,
                    payload,
                    latest_token_usage.clone(),
                ));
                parsed
                    .messages
                    .push(message_from_compacted(line_number, &timestamp, payload));
            }
            "event_msg" => {
                pending_boundary = None;
                let payload_type = string(payload.get("type"));
                if should_include_event_message(&payload_type, coverage) {
                    parsed
                        .messages
                        .push(message_from_event(line_number, &timestamp, payload));
                }
                if payload_type == "token_count" {
                    parsed.stats.token_count_events += 1;
                    latest_token_usage = apply_token_count(&mut parsed.stats, payload);
                } else if payload_type == "context_compacted" {
                    legacy_context_compacted_events.push(parse_legacy_context_compacted(
                        line_number,
                        &timestamp,
                        latest_token_usage.clone(),
                    ));
                }
            }
            "response_item" => {
                pending_boundary = None;
                parsed
                    .messages
                    .push(message_from_response_item(line_number, &timestamp, payload));
            }
            _ if payload_empty => {
                if let Some(boundary) = parse_raw_boundary(line_number, &timestamp, &record) {
                    pending_boundary = Some(boundary);
                    parsed
                        .messages
                        .push(message_from_raw_record(line_number, &timestamp, &record));
                } else if is_raw_compact_summary(&record) {
                    if let Some(event) = parse_raw_compact_summary(
                        line_number,
                        &timestamp,
                        &record,
                        pending_boundary.as_ref(),
                    ) {
                        parsed.compaction_events.push(event);
                    }
                    pending_boundary = None;
                    parsed
                        .messages
                        .push(message_from_raw_record(line_number, &timestamp, &record));
                } else if matches!(record_type.as_str(), "system" | "user" | "assistant") {
                    pending_boundary = None;
                    parsed
                        .messages
                        .push(message_from_raw_record(line_number, &timestamp, &record));
                } else {
                    pending_boundary = None;
                    parsed.messages.push(ParsedMessage::new(
                        line_number,
                        &timestamp,
                        record_type.clone(),
                        record_type,
                        "",
                        "",
                    ));
                }
            }
            _ => {
                pending_boundary = None;
                parsed.messages.push(
                    ParsedMessage::new(
                        line_number,
                        &timestamp,
                        record_type,
                        string(payload.get("type")),
                        "",
                        "",
                    )
                    .with_raw_payload(payload),
                );
            }
        }
    }

    let existing_compaction_events = parsed.compaction_events.clone();
    parsed
        .compaction_events
        .extend(legacy_context_compacted_events.into_iter().filter(|event| {
            !is_duplicate_legacy_context_compacted(event, &existing_compaction_events)
        }));
    parsed
        .compaction_events
        .sort_by_key(|event| event.line_number);
    parsed.stats.message_count = parsed.messages.len();
    Ok(parsed)
}

fn load_records(path: &Path) -> Result<LoadedRecords> {
    if let Some(records) = load_json_document_records(path)? {
        return Ok(records);
    }

    load_jsonl_records(path)
}

fn load_json_document_records(path: &Path) -> Result<Option<LoadedRecords>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    let value: Value = match serde_json::from_reader(reader) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };

    let mut loaded = LoadedRecords::default();
    match value {
        Value::Array(items) => {
            loaded.line_count = items.len();
            for (index, item) in items.into_iter().enumerate() {
                push_normalized_record(&mut loaded, index + 1, item);
            }
        }
        item => {
            loaded.line_count = 1;
            push_normalized_record(&mut loaded, 1, item);
        }
    }

    Ok(Some(loaded))
}

fn load_jsonl_records(path: &Path) -> Result<LoadedRecords> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut loaded = LoadedRecords::default();

    for (line_index, line) in reader.lines().enumerate() {
        let line_number = line_index + 1;
        loaded.line_count += 1;
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => push_normalized_record(&mut loaded, line_number, value),
            Err(_) => loaded.bad_lines += 1,
        }
    }

    Ok(loaded)
}

fn push_normalized_record(loaded: &mut LoadedRecords, line_number: usize, value: Value) {
    match value {
        Value::Object(record) => loaded.records.push(JsonRecord {
            line_number,
            record,
        }),
        Value::String(raw) => match serde_json::from_str::<Value>(raw.trim()) {
            Ok(Value::Object(record)) => loaded.records.push(JsonRecord {
                line_number,
                record,
            }),
            _ => loaded.bad_lines += 1,
        },
        _ => loaded.bad_lines += 1,
    }
}

fn response_item_coverage(records: &[JsonRecord]) -> ResponseItemCoverage {
    let mut coverage = ResponseItemCoverage::default();
    for item in records {
        if string(item.record.get("type")) != "response_item" {
            continue;
        }
        let payload = item.record.get("payload").and_then(Value::as_object);
        let Some(payload) = payload else {
            continue;
        };
        match string(payload.get("type")).as_str() {
            "message" => match string(payload.get("role")).as_str() {
                "user" => coverage.user_messages = true,
                "assistant" => coverage.assistant_messages = true,
                _ => {}
            },
            "reasoning" => coverage.reasoning = true,
            _ => {}
        }
    }
    coverage
}

fn should_include_event_message(kind: &str, coverage: ResponseItemCoverage) -> bool {
    match kind {
        "user_message" => !coverage.user_messages,
        "agent_message" => !coverage.assistant_messages,
        "agent_reasoning" => !coverage.reasoning,
        _ => true,
    }
}

fn apply_session_meta(
    metadata: &mut SessionMetadata,
    payload: &Map<String, Value>,
    timestamp: &str,
) {
    assign_if_present(&mut metadata.session_id, string(payload.get("id")));
    assign_if_present(&mut metadata.cwd, string(payload.get("cwd")));
    assign_if_present(
        &mut metadata.cli_version,
        string(payload.get("cli_version")),
    );
    assign_if_present(
        &mut metadata.model_provider,
        string(payload.get("model_provider")),
    );
    if !timestamp.is_empty() {
        metadata.started_at = timestamp.to_string();
    } else {
        assign_if_present(&mut metadata.started_at, string(payload.get("timestamp")));
    }
}

fn parse_turn_context(
    line_number: usize,
    timestamp: &str,
    payload: &Map<String, Value>,
    latest_token_usage: Option<TokenUsage>,
) -> Option<CompactionEvent> {
    let summary = summary_text(payload.get("summary"));
    if summary.is_empty() {
        return None;
    }

    let policy = payload.get("truncation_policy").and_then(Value::as_object);

    Some(CompactionEvent {
        line_number,
        timestamp: timestamp.to_string(),
        turn_id: string(payload.get("turn_id")),
        summary,
        truncation_mode: policy.map(|p| string(p.get("mode"))).unwrap_or_default(),
        truncation_limit: policy.and_then(|p| int_or_none(p.get("limit"))),
        token_usage: latest_token_usage,
        source: "turn_context".to_string(),
        boundary_line_number: None,
        trigger: String::new(),
    })
}

fn parse_raw_boundary(
    line_number: usize,
    timestamp: &str,
    record: &Map<String, Value>,
) -> Option<PendingBoundary> {
    if string(record.get("type")) != "system" || string(record.get("subtype")) != "compact_boundary"
    {
        return None;
    }

    let metadata = record
        .get("compactMetadata")
        .or_else(|| record.get("compact_metadata"))
        .and_then(Value::as_object);

    let token_count = metadata.and_then(|metadata| {
        first_int(
            metadata,
            &[
                "preCompactTokens",
                "pre_compact_tokens",
                "tokensBefore",
                "tokens_before",
                "totalTokens",
                "total_tokens",
            ],
        )
    });
    let token_usage = token_count.map(|total_tokens| TokenUsage {
        total_tokens,
        ..TokenUsage::default()
    });

    Some(PendingBoundary {
        line_number,
        timestamp: timestamp.to_string(),
        trigger: metadata
            .map(|m| string(m.get("trigger")))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| string(record.get("trigger"))),
        token_usage,
    })
}

fn parse_raw_compact_summary(
    line_number: usize,
    timestamp: &str,
    record: &Map<String, Value>,
    boundary: Option<&PendingBoundary>,
) -> Option<CompactionEvent> {
    let summary = [
        summary_text(record.get("summary")),
        summary_text(record.get("compactSummary")),
        summary_text(record.get("compact_summary")),
        summary_text_from_string(message_text(record.get("message"))),
        summary_text(record.get("content")),
    ]
    .into_iter()
    .find(|summary| !summary.is_empty())
    .unwrap_or_default();

    if summary.is_empty() {
        return None;
    }

    Some(CompactionEvent {
        line_number,
        timestamp: timestamp.to_string(),
        turn_id: string(record.get("uuid")).or_else_empty(string(record.get("id"))),
        summary,
        truncation_mode: String::new(),
        truncation_limit: None,
        token_usage: boundary.and_then(|boundary| boundary.token_usage.clone()),
        source: if boundary.is_some() {
            "boundary_summary"
        } else {
            "compact_summary"
        }
        .to_string(),
        boundary_line_number: boundary.map(|boundary| boundary.line_number),
        trigger: boundary
            .map(|boundary| boundary.trigger.clone())
            .filter(|trigger| !trigger.is_empty())
            .unwrap_or_else(|| string(record.get("trigger"))),
    })
}

fn parse_compacted(
    line_number: usize,
    timestamp: &str,
    payload: &Map<String, Value>,
    latest_token_usage: Option<TokenUsage>,
) -> CompactionEvent {
    let replacement_history_len = replacement_history_len(payload);
    let summary = summary_text(payload.get("message"))
        .or_else_empty(summary_from_replacement_history(
            payload.get("replacement_history"),
        ))
        .or_else_empty(format!(
            "Compacted history checkpoint (replacement history items: {replacement_history_len})."
        ));

    CompactionEvent {
        line_number,
        timestamp: timestamp.to_string(),
        turn_id: string(payload.get("turn_id")),
        summary,
        truncation_mode: String::new(),
        truncation_limit: None,
        token_usage: latest_token_usage,
        source: "rollout_compacted".to_string(),
        boundary_line_number: None,
        trigger: String::new(),
    }
}

fn parse_legacy_context_compacted(
    line_number: usize,
    timestamp: &str,
    latest_token_usage: Option<TokenUsage>,
) -> CompactionEvent {
    CompactionEvent {
        line_number,
        timestamp: timestamp.to_string(),
        turn_id: String::new(),
        summary: "legacy context_compacted event.".to_string(),
        truncation_mode: String::new(),
        truncation_limit: None,
        token_usage: latest_token_usage,
        source: "context_compacted_event".to_string(),
        boundary_line_number: None,
        trigger: String::new(),
    }
}

fn is_duplicate_legacy_context_compacted(
    legacy_event: &CompactionEvent,
    events: &[CompactionEvent],
) -> bool {
    events.iter().any(|event| {
        event.source == "rollout_compacted"
            && event.line_number.abs_diff(legacy_event.line_number) <= 3
    })
}

fn is_raw_compact_summary(record: &Map<String, Value>) -> bool {
    string(record.get("type")) == "user"
        && record
            .get("isCompactSummary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn message_from_turn_context(
    line_number: usize,
    timestamp: &str,
    payload: &Map<String, Value>,
) -> ParsedMessage {
    let policy_text = payload
        .get("truncation_policy")
        .and_then(Value::as_object)
        .map(|policy| {
            let mode = string(policy.get("mode"));
            let limit = string(policy.get("limit"));
            if mode.is_empty() && limit.is_empty() {
                String::new()
            } else {
                format!("{mode}:{limit}")
            }
        })
        .unwrap_or_default();

    ParsedMessage::new(
        line_number,
        timestamp,
        "turn_context",
        "turn_context",
        "system",
        policy_text,
    )
    .with_raw_payload(payload)
}

fn message_from_event(
    line_number: usize,
    timestamp: &str,
    payload: &Map<String, Value>,
) -> ParsedMessage {
    let kind = string(payload.get("type"));
    let (role, content) = match kind.as_str() {
        "user_message" => ("user".to_string(), string(payload.get("message"))),
        "agent_message" => ("assistant".to_string(), string(payload.get("message"))),
        "agent_reasoning" => (
            "assistant".to_string(),
            string(payload.get("text")).or_else_empty(string(payload.get("message"))),
        ),
        "exec_command_end" => {
            let status =
                string(payload.get("status")).or_else_empty(string(payload.get("exit_code")));
            let command_text = match payload.get("command") {
                Some(Value::Array(parts)) => parts
                    .iter()
                    .map(|part| string(Some(part)))
                    .collect::<Vec<_>>()
                    .join(" "),
                other => string(other),
            };
            (
                "tool".to_string(),
                format!("{command_text} {status}").trim().to_string(),
            )
        }
        "token_count" => {
            let total_tokens = payload
                .get("info")
                .and_then(Value::as_object)
                .and_then(|info| info.get("total_token_usage"))
                .and_then(Value::as_object)
                .map(|usage| string(usage.get("total_tokens")))
                .unwrap_or_default();
            let content = if total_tokens.is_empty() {
                String::new()
            } else {
                format!("tokens={total_tokens}")
            };
            ("system".to_string(), content)
        }
        _ => (
            "system".to_string(),
            string(payload.get("message")).or_else_empty(string(payload.get("text"))),
        ),
    };

    ParsedMessage::new(line_number, timestamp, "event_msg", kind, role, content)
        .with_raw_payload(payload)
}

fn message_from_response_item(
    line_number: usize,
    timestamp: &str,
    payload: &Map<String, Value>,
) -> ParsedMessage {
    let kind = string(payload.get("type"));
    let (role, content, request_body, response_body) = match kind.as_str() {
        "function_call" => (
            "tool_call".to_string(),
            function_call_content(payload),
            string(payload.get("arguments")),
            String::new(),
        ),
        "custom_tool_call" => (
            "tool_call".to_string(),
            custom_tool_call_content(payload),
            string(payload.get("input")),
            String::new(),
        ),
        "function_call_output" | "custom_tool_call_output" => (
            "tool".to_string(),
            tool_output_content(payload),
            String::new(),
            raw_output_body(payload),
        ),
        "reasoning" => (
            "assistant".to_string(),
            summary_text(payload.get("summary"))
                .or_else_empty(string(payload.get("text")))
                .or_else_empty(content_text(payload.get("content"))),
            String::new(),
            String::new(),
        ),
        _ => (
            string(payload.get("role")),
            content_text(payload.get("content")),
            String::new(),
            String::new(),
        ),
    };

    ParsedMessage::new(line_number, timestamp, "response_item", kind, role, content)
        .with_request_body(request_body)
        .with_response_body(response_body)
        .with_raw_payload(payload)
}

fn function_call_content(payload: &Map<String, Value>) -> String {
    let name = string(payload.get("name")).or_else_empty("tool".to_string());
    let arguments = string(payload.get("arguments"));
    let detail = exec_command_from_arguments(&arguments)
        .or_else(|| (!arguments.is_empty()).then(|| format_json_if_possible(&arguments)))
        .unwrap_or_default();

    if detail.is_empty() {
        name
    } else {
        format!("{name}: {detail}")
    }
}

fn custom_tool_call_content(payload: &Map<String, Value>) -> String {
    let name = string(payload.get("name")).or_else_empty("tool".to_string());
    let input = string(payload.get("input"));
    if input.is_empty() {
        name
    } else {
        format!("{name}: {input}")
    }
}

fn tool_output_content(payload: &Map<String, Value>) -> String {
    let output = match payload.get("output") {
        Some(Value::String(value)) => format_json_if_possible(value),
        Some(value) => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        None => String::new(),
    };

    output
        .or_else_empty(string(payload.get("call_id")))
        .or_else_empty("[empty output]".to_string())
}

fn raw_output_body(payload: &Map<String, Value>) -> String {
    match payload.get("output") {
        Some(Value::String(value)) => value.clone(),
        Some(value) => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        None => String::new(),
    }
}

fn exec_command_from_arguments(arguments: &str) -> Option<String> {
    let value: Value = serde_json::from_str(arguments).ok()?;
    let object = value.as_object()?;
    string(object.get("cmd")).then_non_empty()
}

fn format_json_if_possible(value: &str) -> String {
    serde_json::from_str::<Value>(value)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| value.to_string())
}

fn format_object_json(value: &Map<String, Value>) -> String {
    serde_json::to_string_pretty(&Value::Object(value.clone())).unwrap_or_default()
}

fn message_from_raw_record(
    line_number: usize,
    timestamp: &str,
    record: &Map<String, Value>,
) -> ParsedMessage {
    let record_type = string(record.get("type"));
    let subtype = string(record.get("subtype"));
    let mut content =
        message_text(record.get("message")).or_else_empty(summary_text(record.get("content")));
    if subtype == "compact_boundary" {
        let trigger = record
            .get("compactMetadata")
            .and_then(Value::as_object)
            .map(|metadata| string(metadata.get("trigger")))
            .unwrap_or_default();
        content = format!("compact boundary {trigger}").trim().to_string();
    }

    ParsedMessage::new(
        line_number,
        timestamp,
        record_type.clone(),
        subtype.or_else_empty(record_type.clone()),
        record_type,
        content,
    )
    .with_raw_payload(record)
}

fn message_from_compacted(
    line_number: usize,
    timestamp: &str,
    payload: &Map<String, Value>,
) -> ParsedMessage {
    let replacement_history_len = replacement_history_len(payload);
    let summary_length = summary_text(payload.get("message")).chars().count();

    ParsedMessage::new(
        line_number,
        timestamp,
        "compacted",
        "compacted",
        "system",
        format!("replacement_history={replacement_history_len} summary_chars={summary_length}"),
    )
    .with_raw_payload(payload)
}

fn apply_token_count(
    stats: &mut ConversationStats,
    payload: &Map<String, Value>,
) -> Option<TokenUsage> {
    let info = payload.get("info")?.as_object()?;
    if let Some(window) = int_or_none(info.get("model_context_window")) {
        stats.model_context_window = window;
    }

    let usage = token_usage(info.get("total_token_usage"))?;
    stats.input_tokens = usage.input_tokens;
    stats.cached_input_tokens = usage.cached_input_tokens;
    stats.output_tokens = usage.output_tokens;
    stats.reasoning_output_tokens = usage.reasoning_output_tokens;
    stats.total_tokens = usage.total_tokens;
    Some(usage)
}

fn replacement_history_len(payload: &Map<String, Value>) -> usize {
    payload
        .get("replacement_history")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

fn summary_from_replacement_history(value: Option<&Value>) -> String {
    let Some(items) = value.and_then(Value::as_array) else {
        return String::new();
    };

    items
        .iter()
        .rev()
        .filter_map(Value::as_object)
        .find_map(|item| {
            let kind = string(item.get("type"));
            if kind == "compaction" || kind == "compaction_summary" {
                summary_text(item.get("text"))
                    .or_else_empty(summary_text(item.get("summary")))
                    .or_else_empty(summary_text(item.get("content")))
                    .then_non_empty()
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn token_usage(value: Option<&Value>) -> Option<TokenUsage> {
    let value = value?.as_object()?;
    Some(TokenUsage {
        input_tokens: int(value.get("input_tokens")),
        cached_input_tokens: int(value.get("cached_input_tokens")),
        output_tokens: int(value.get("output_tokens")),
        reasoning_output_tokens: int(value.get("reasoning_output_tokens")),
        total_tokens: int(value.get("total_tokens")),
    })
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(value) => Some(value.clone()),
                Value::Object(object) => {
                    let text =
                        string(object.get("text")).or_else_empty(string(object.get("summary")));
                    (!text.is_empty()).then_some(text)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn message_text(message: Option<&Value>) -> String {
    match message {
        Some(Value::Object(object)) => content_text(object.get("content"))
            .or_else_empty(string(object.get("text")))
            .or_else_empty(string(object.get("summary"))),
        other => content_text(other),
    }
}

fn summary_text(value: Option<&Value>) -> String {
    let summary = match value {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Array(_)) => content_text(value).trim().to_string(),
        Some(Value::Object(object)) => string(object.get("text"))
            .or_else_empty(string(object.get("summary")))
            .trim()
            .to_string(),
        _ => String::new(),
    };

    summary_text_from_string(summary)
}

fn summary_text_from_string(summary: String) -> String {
    let summary = summary.trim().to_string();
    if EMPTY_SUMMARY_VALUES
        .iter()
        .any(|empty| summary.eq_ignore_ascii_case(empty))
    {
        String::new()
    } else {
        summary
    }
}

fn update_time_bounds(stats: &mut ConversationStats, timestamp: &str) {
    if stats.first_timestamp.is_empty() || timestamp < stats.first_timestamp.as_str() {
        stats.first_timestamp = timestamp.to_string();
    }
    if stats.last_timestamp.is_empty() || timestamp > stats.last_timestamp.as_str() {
        stats.last_timestamp = timestamp.to_string();
    }
}

fn string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn int(value: Option<&Value>) -> i64 {
    int_or_none(value).unwrap_or(0)
}

fn int_or_none(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(value)) => value
            .as_i64()
            .or_else(|| value.as_u64().map(|value| value as i64)),
        Some(Value::String(value)) => value.parse().ok(),
        _ => None,
    }
}

fn first_int(values: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| int_or_none(values.get(*key)))
}

fn assign_if_present(target: &mut String, value: String) {
    if !value.is_empty() {
        *target = value;
    }
}

trait EmptyFallback {
    fn or_else_empty(self, fallback: String) -> String;
    fn then_non_empty(self) -> Option<String>;
}

impl EmptyFallback for String {
    fn or_else_empty(self, fallback: String) -> String {
        if self.is_empty() {
            fallback
        } else {
            self
        }
    }

    fn then_non_empty(self) -> Option<String> {
        (!self.is_empty()).then_some(self)
    }
}
