use crate::parser::{discover_sessions, parse_jsonl, CompactionEvent, ParsedSession};
use crate::tui::TuiDisplayMode;
use anyhow::Result;
use clap::{ArgAction, CommandFactory, Parser, ValueEnum};
use serde::Serialize;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "cxv",
    about = "Inspect Codex JSONL sessions for context summaries, token usage, and truncation signals."
)]
pub struct Args {
    /// Codex JSONL file to inspect.
    pub file: Option<PathBuf>,

    /// Scan ~/.codex/sessions for JSONL files.
    #[arg(long)]
    pub scan: bool,

    /// Codex home root, defaults to ~/.codex.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Include ~/.codex/archived_sessions.
    #[arg(long)]
    pub include_archived: bool,

    /// Print context summary events for a JSONL session.
    #[arg(long)]
    pub summary: Option<PathBuf>,

    /// Emit structured JSON.
    #[arg(long)]
    pub json: bool,

    /// Launch the interactive terminal viewer. Pass a positional FILE to open it directly.
    #[arg(long)]
    pub tui: bool,

    /// TUI history display mode.
    #[arg(long, value_enum, default_value_t = DisplayModeArg::Tidy)]
    pub mode: DisplayModeArg,

    /// Enable raw request/response body popups in the TUI.
    #[arg(long)]
    pub raw_bodies: bool,

    /// Print version information.
    #[arg(short = 'v', long = "version", action = ArgAction::SetTrue)]
    pub version: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DisplayModeArg {
    Tidy,
    Verbose,
}

impl From<DisplayModeArg> for TuiDisplayMode {
    fn from(value: DisplayModeArg) -> Self {
        match value {
            DisplayModeArg::Tidy => Self::Tidy,
            DisplayModeArg::Verbose => Self::Verbose,
        }
    }
}

#[derive(Debug, Serialize)]
struct ScanRow {
    path: String,
    session_id: String,
    cwd: String,
    started_at: String,
    last_timestamp: String,
    lines: usize,
    bad_lines: usize,
    messages: usize,
    compactions: usize,
    token_count_events: usize,
    total_tokens: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    model_context_window: i64,
}

#[derive(Debug, Serialize)]
struct SessionOutput {
    #[serde(flatten)]
    row: ScanRow,
    compaction_events: Vec<EventOutput>,
}

#[derive(Debug, Serialize)]
struct EventOutput {
    line: usize,
    boundary_line: Option<usize>,
    timestamp: String,
    turn_id: String,
    source: String,
    trigger: String,
    summary_length: usize,
    summary: String,
    truncation_mode: String,
    truncation_limit: Option<i64>,
    tokens_before: i64,
}

pub fn main_entry() -> i32 {
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    let args = match Args::try_parse_from(raw_args.clone()) {
        Ok(args) => args,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };

    if should_launch_tui(&args, raw_args.len()) {
        return match crate::tui::launch(
            args.root.as_deref(),
            args.include_archived,
            args.file.as_deref(),
            args.mode.into(),
            args.raw_bodies,
        ) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("cxv: {error:#}");
                1
            }
        };
    }

    match run(args) {
        Ok(output) => {
            print!("{output}");
            0
        }
        Err(error) => {
            eprintln!("cxv: {error:#}");
            1
        }
    }
}

fn should_launch_tui(args: &Args, arg_count: usize) -> bool {
    !args.version
        && (args.tui || arg_count == 1)
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
}

pub fn run_from<I, T>(args: I) -> Result<String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = Args::parse_from(args);
    run(args)
}

pub fn run(args: Args) -> Result<String> {
    if args.version {
        return Ok(crate::version_line());
    }

    if args.tui {
        return Ok(
            "Interactive TUI requires a terminal. Run `cxv --tui` from a TTY.\n".to_string(),
        );
    }

    if let Some(summary) = args.summary {
        let parsed = parse_jsonl(summary)?;
        if args.json {
            return Ok(format!(
                "{}\n",
                serde_json::to_string_pretty(&session_output(&parsed))?
            ));
        }
        return Ok(print_summary(&parsed));
    }

    if let Some(file) = args.file {
        let parsed = parse_jsonl(file)?;
        if args.json {
            return Ok(format!(
                "{}\n",
                serde_json::to_string_pretty(&session_output(&parsed))?
            ));
        }
        return Ok(print_session(&parsed));
    }

    if args.scan {
        let paths = discover_sessions(args.root.as_deref(), args.include_archived)?;
        let sessions = paths.iter().map(parse_jsonl).collect::<Result<Vec<_>>>()?;
        let rows = sessions.iter().map(scan_row).collect::<Vec<_>>();
        if args.json {
            return Ok(format!("{}\n", serde_json::to_string_pretty(&rows)?));
        }
        return Ok(print_scan(&rows));
    }

    let mut command = Args::command();
    let mut help = Vec::new();
    command.write_help(&mut help)?;
    Ok(format!("{}\n", String::from_utf8(help)?))
}

fn scan_row(session: &ParsedSession) -> ScanRow {
    let stats = &session.stats;
    let metadata = &session.metadata;
    ScanRow {
        path: metadata.path.display().to_string(),
        session_id: if metadata.session_id.is_empty() {
            metadata
                .path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string()
        } else {
            metadata.session_id.clone()
        },
        cwd: metadata.cwd.clone(),
        started_at: if metadata.started_at.is_empty() {
            stats.first_timestamp.clone()
        } else {
            metadata.started_at.clone()
        },
        last_timestamp: stats.last_timestamp.clone(),
        lines: stats.line_count,
        bad_lines: stats.bad_lines,
        messages: stats.message_count,
        compactions: session.compaction_events.len(),
        token_count_events: stats.token_count_events,
        total_tokens: stats.total_tokens,
        input_tokens: stats.input_tokens,
        cached_input_tokens: stats.cached_input_tokens,
        output_tokens: stats.output_tokens,
        reasoning_output_tokens: stats.reasoning_output_tokens,
        model_context_window: stats.model_context_window,
    }
}

fn session_output(session: &ParsedSession) -> SessionOutput {
    SessionOutput {
        row: scan_row(session),
        compaction_events: session.compaction_events.iter().map(event_output).collect(),
    }
}

fn event_output(event: &CompactionEvent) -> EventOutput {
    EventOutput {
        line: event.line_number,
        boundary_line: event.boundary_line_number,
        timestamp: event.timestamp.clone(),
        turn_id: event.turn_id.clone(),
        source: event.source.clone(),
        trigger: event.trigger.clone(),
        summary_length: event.summary_length(),
        summary: event.summary.clone(),
        truncation_mode: event.truncation_mode.clone(),
        truncation_limit: event.truncation_limit,
        tokens_before: event
            .token_usage
            .as_ref()
            .map(|usage| usage.total_tokens)
            .unwrap_or(0),
    }
}

fn print_scan(rows: &[ScanRow]) -> String {
    if rows.is_empty() {
        return "No Codex sessions found.\n".to_string();
    }

    let headers = [
        "Session",
        "Compactions",
        "Lines",
        "Tokens",
        "Context",
        "CWD",
    ];
    let table_rows = rows
        .iter()
        .map(|row| {
            vec![
                short(&row.session_id, 18),
                row.compactions.to_string(),
                row.lines.to_string(),
                compact_number(row.total_tokens),
                compact_number(row.model_context_window),
                short(&row.cwd, 44),
            ]
        })
        .collect::<Vec<_>>();

    print_table(&headers, &table_rows)
}

fn print_session(session: &ParsedSession) -> String {
    let row = scan_row(session);
    let mut output = String::new();
    output.push_str(&format!("Session: {}\n", row.session_id));
    output.push_str(&format!("Path: {}\n", row.path));
    if !row.cwd.is_empty() {
        output.push_str(&format!("CWD: {}\n", row.cwd));
    }
    output.push_str(&format!(
        "Lines: {}  Messages: {}  Compactions: {}  Tokens: {}\n\n",
        row.lines,
        row.messages,
        row.compactions,
        compact_number(row.total_tokens)
    ));
    output.push_str(&print_summary(session));
    output
}

fn print_summary(session: &ParsedSession) -> String {
    if session.compaction_events.is_empty() {
        return "No Codex context summary events found.\n".to_string();
    }

    let mut output = String::new();
    for (index, event) in session.compaction_events.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }

        let mut heading = format!("#{} line {}", index + 1, event.line_number);
        if let Some(boundary_line) = event.boundary_line_number {
            heading.push_str(&format!(" boundary {boundary_line}"));
        }
        if !event.turn_id.is_empty() {
            heading.push_str(&format!(" turn {}", event.turn_id));
        }
        output.push_str(&format!("{heading}\n"));

        if !event.timestamp.is_empty() {
            output.push_str(&format!("timestamp: {}\n", event.timestamp));
        }
        if !event.trigger.is_empty() {
            output.push_str(&format!("trigger: {}\n", event.trigger));
        }

        let policy = if let Some(limit) = event.truncation_limit {
            if event.truncation_mode.is_empty() {
                limit.to_string()
            } else {
                format!("{}:{limit}", event.truncation_mode)
            }
        } else {
            event.truncation_mode.clone()
        };
        if !policy.is_empty() {
            output.push_str(&format!("truncation: {policy}\n"));
        }

        let tokens = event
            .token_usage
            .as_ref()
            .map(|usage| usage.total_tokens)
            .unwrap_or(0);
        if tokens != 0 {
            output.push_str(&format!("tokens before: {}\n", compact_number(tokens)));
        }
        output.push_str(&event.summary);
        output.push('\n');
    }
    output
}

fn print_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(column, header)| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
                .max(header.chars().count())
        })
        .collect::<Vec<_>>();

    let mut output = String::new();
    output.push_str(&join_padded(
        headers.iter().map(|value| value.to_string()),
        &widths,
    ));
    output.push('\n');
    output.push_str(&join_padded(
        widths.iter().map(|width| "-".repeat(*width)),
        &widths,
    ));
    output.push('\n');
    for row in rows {
        output.push_str(&join_padded(row.iter().cloned(), &widths));
        output.push('\n');
    }
    output
}

fn join_padded(values: impl Iterator<Item = String>, widths: &[usize]) -> String {
    values
        .enumerate()
        .map(|(index, value)| format!("{value:<width$}", width = widths[index]))
        .collect::<Vec<_>>()
        .join("  ")
}

fn short(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    if limit <= 3 {
        return value.chars().take(limit).collect();
    }
    format!("{}...", value.chars().take(limit - 3).collect::<String>())
}

fn compact_number(value: i64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}
