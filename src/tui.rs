use crate::parser::{
    discover_sessions, parse_jsonl, CompactionEvent, ParsedMessage, ParsedSession,
};
use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
    },
    Frame, Terminal,
};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone)]
pub struct TuiSession {
    pub path: PathBuf,
    pub session_id: String,
    pub cwd: String,
    pub started_at: String,
    pub last_timestamp: String,
    pub lines: usize,
    pub messages: usize,
    pub compactions: usize,
    pub total_tokens: i64,
    pub model_context_window: i64,
    parsed: ParsedSession,
}

#[derive(Debug, Clone)]
pub struct TuiModel {
    pub sessions: Vec<TuiSession>,
    pub selected_session: usize,
}

#[derive(Debug, Clone)]
pub struct TuiState {
    pub model: TuiModel,
    pub selected_message: usize,
    pub show_summaries: bool,
    pub show_token_events: bool,
}

pub fn build_tui_model(
    root: Option<&Path>,
    include_archived: bool,
    initial_file: Option<&Path>,
) -> Result<TuiModel> {
    let mut paths = if let Some(initial_file) = initial_file {
        match discover_sessions(root, include_archived) {
            Ok(mut discovered) => {
                if !discovered.iter().any(|path| path == initial_file) {
                    discovered.push(initial_file.to_path_buf());
                }
                discovered
            }
            Err(_) => vec![initial_file.to_path_buf()],
        }
    } else {
        discover_sessions(root, include_archived)?
    };
    paths.sort();
    paths.dedup();

    let mut sessions = paths
        .iter()
        .map(|path| {
            parse_jsonl(path)
                .with_context(|| format!("failed to parse {}", path.display()))
                .map(session_row)
        })
        .collect::<Result<Vec<_>>>()?;

    sessions.sort_by(|left, right| {
        right
            .last_timestamp
            .cmp(&left.last_timestamp)
            .then_with(|| right.started_at.cmp(&left.started_at))
            .then_with(|| left.path.cmp(&right.path))
    });

    let selected_session = initial_file
        .and_then(|initial| sessions.iter().position(|session| session.path == initial))
        .unwrap_or(0);

    Ok(TuiModel {
        sessions,
        selected_session,
    })
}

pub fn launch(
    root: Option<&Path>,
    include_archived: bool,
    initial_file: Option<&Path>,
) -> Result<()> {
    let model = build_tui_model(root, include_archived, initial_file)?;
    let mut state = TuiState::new(model);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_terminal(&mut terminal, &mut state);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

impl TuiState {
    pub fn new(model: TuiModel) -> Self {
        Self {
            selected_message: 0,
            model,
            show_summaries: false,
            show_token_events: false,
        }
    }

    pub fn selected_message_line(&self) -> Option<usize> {
        self.visible_messages()
            .get(self.selected_message)
            .map(|message| message.line_number)
    }

    pub fn compaction_summary_text(&self) -> String {
        let Some(session) = self.current_session() else {
            return "No session selected.".to_string();
        };

        if session.parsed.compaction_events.is_empty() {
            return "No Codex context summary events found.".to_string();
        }

        let mut output = String::new();
        for (index, event) in session.parsed.compaction_events.iter().enumerate() {
            if index > 0 {
                output.push_str("\n\n");
            }
            output.push_str(&format!("COMPACTION {}\n", index + 1));
            output.push_str(&format!("line {}", event.line_number));
            if let Some(boundary_line) = event.boundary_line_number {
                output.push_str(&format!(" boundary {boundary_line}"));
            }
            if !event.timestamp.is_empty() {
                output.push_str(&format!(" @ {}", event.timestamp));
            }
            output.push('\n');
            if !event.trigger.is_empty() {
                output.push_str(&format!("trigger: {}\n", event.trigger));
            }
            if let Some(tokens) = event.token_usage.as_ref().map(|usage| usage.total_tokens) {
                if tokens > 0 {
                    output.push_str(&format!("tokens before: {}\n", compact_number(tokens)));
                }
            }
            if let Some(limit) = event.truncation_limit {
                if event.truncation_mode.is_empty() {
                    output.push_str(&format!("truncation: {limit}\n"));
                } else {
                    output.push_str(&format!("truncation: {}:{limit}\n", event.truncation_mode));
                }
            } else if !event.truncation_mode.is_empty() {
                output.push_str(&format!("truncation: {}\n", event.truncation_mode));
            }
            output.push('\n');
            output.push_str(&event.summary);
        }
        output
    }

    pub fn jump_next_compaction(&mut self) {
        let rows = self.compaction_rows();
        if rows.is_empty() {
            return;
        }
        let next = rows
            .iter()
            .copied()
            .find(|row| *row > self.selected_message)
            .unwrap_or(rows[0]);
        self.selected_message = next;
        self.show_summaries = false;
    }

    pub fn jump_previous_compaction(&mut self) {
        let rows = self.compaction_rows();
        if rows.is_empty() {
            return;
        }
        let previous = rows
            .iter()
            .rev()
            .copied()
            .find(|row| *row < self.selected_message)
            .unwrap_or_else(|| *rows.last().expect("non-empty rows"));
        self.selected_message = previous;
        self.show_summaries = false;
    }

    fn current_session(&self) -> Option<&TuiSession> {
        self.model.sessions.get(self.model.selected_session)
    }

    fn visible_messages(&self) -> Vec<&ParsedMessage> {
        self.current_session()
            .map(|session| {
                session
                    .parsed
                    .messages
                    .iter()
                    .filter(|message| self.show_token_events || message.kind != "token_count")
                    .collect()
            })
            .unwrap_or_default()
    }

    fn compaction_rows(&self) -> Vec<usize> {
        let Some(session) = self.current_session() else {
            return Vec::new();
        };
        let lines = compaction_lines(&session.parsed.compaction_events);
        self.visible_messages()
            .iter()
            .enumerate()
            .filter_map(|(index, message)| lines.contains(&message.line_number).then_some(index))
            .collect()
    }

    fn selected_detail_text(&self) -> String {
        if self.show_summaries {
            return self.compaction_summary_text();
        }

        let Some(message) = self.visible_messages().get(self.selected_message).copied() else {
            return "No message selected.".to_string();
        };

        let mut output = String::new();
        output.push_str(&format!(
            "{} line {}\n",
            display_kind(message),
            message.line_number
        ));
        if !message.timestamp.is_empty() {
            output.push_str(&format!("time: {}\n", message.timestamp));
        }
        if !message.role.is_empty() {
            output.push_str(&format!("role: {}\n", message.role));
        }
        if !message.kind.is_empty() {
            output.push_str(&format!("kind: {}\n", message.kind));
        }

        let events = self.events_for_line(message.line_number);
        for event in events {
            output.push('\n');
            output.push_str(&format!("COMPACTION EVENT line {}\n", event.line_number));
            if let Some(boundary_line) = event.boundary_line_number {
                output.push_str(&format!("boundary line: {boundary_line}\n"));
            }
            if !event.trigger.is_empty() {
                output.push_str(&format!("trigger: {}\n", event.trigger));
            }
            if let Some(tokens) = event.token_usage.as_ref().map(|usage| usage.total_tokens) {
                if tokens > 0 {
                    output.push_str(&format!("tokens before: {}\n", compact_number(tokens)));
                }
            }
            output.push_str(&format!("summary chars: {}\n\n", event.summary_length()));
            output.push_str(&event.summary);
            output.push('\n');
        }

        if !message.content.is_empty() {
            output.push('\n');
            output.push_str(&message.content);
        }
        if output.chars().count() > 20_000 {
            format!(
                "{}...\n\n(truncated)",
                output.chars().take(20_000).collect::<String>()
            )
        } else {
            output
        }
    }

    fn events_for_line(&self, line_number: usize) -> Vec<&CompactionEvent> {
        self.current_session()
            .map(|session| {
                session
                    .parsed
                    .compaction_events
                    .iter()
                    .filter(|event| {
                        event.line_number == line_number
                            || event.boundary_line_number == Some(line_number)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn move_message(&mut self, delta: isize) {
        let count = self.visible_messages().len();
        if count == 0 {
            self.selected_message = 0;
            return;
        }
        self.selected_message = move_index(self.selected_message, delta, count);
        self.show_summaries = false;
    }

    fn page_message(&mut self, delta: isize) {
        self.move_message(delta * 10);
    }

    fn move_session(&mut self, delta: isize) {
        let count = self.model.sessions.len();
        if count == 0 {
            self.model.selected_session = 0;
            self.selected_message = 0;
            return;
        }
        self.model.selected_session = move_index(self.model.selected_session, delta, count);
        self.selected_message = 0;
        self.show_summaries = false;
    }
}

fn run_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
) -> Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, state))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(state, key) {
                    return Ok(());
                }
            }
        }
    }
}

fn handle_key(state: &mut TuiState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char('j') | KeyCode::Down => state.move_message(1),
        KeyCode::Char('k') | KeyCode::Up => state.move_message(-1),
        KeyCode::PageDown => state.page_message(1),
        KeyCode::PageUp => state.page_message(-1),
        KeyCode::Char('h') | KeyCode::Left => state.move_session(-1),
        KeyCode::Char('l') | KeyCode::Right => state.move_session(1),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::SHIFT) => {
            state.jump_previous_compaction();
        }
        KeyCode::Char('C') => state.jump_previous_compaction(),
        KeyCode::Char('c') => state.jump_next_compaction(),
        KeyCode::Char('s') => state.show_summaries = !state.show_summaries,
        KeyCode::Char('t') => {
            state.show_token_events = !state.show_token_events;
            state.selected_message = 0;
        }
        _ => {}
    }
    false
}

fn draw(frame: &mut Frame<'_>, state: &TuiState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_title(frame, root[0]);
    draw_footer(frame, root[2], state);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(38), Constraint::Min(40)])
        .split(root[1]);

    draw_sessions(frame, body[0], state);
    draw_session_content(frame, body[1], state);
}

fn draw_title(frame: &mut Frame<'_>, area: Rect) {
    let title = Paragraph::new("cxv - Codex Compaction Viewer").style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(title, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let token_events = if state.show_token_events {
        "token events:on"
    } else {
        "token events:off"
    };
    let text = format!(
        "q quit | h/l sessions | j/k messages | c/C compactions | s summaries | t {token_events}"
    );
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_sessions(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let items = if state.model.sessions.is_empty() {
        vec![ListItem::new("No Codex sessions found")]
    } else {
        state
            .model
            .sessions
            .iter()
            .map(|session| {
                let title = format!(
                    "{}  compactions:{}",
                    short(&session.session_id, 18),
                    session.compactions
                );
                let cwd = if session.cwd.is_empty() {
                    session.path.display().to_string()
                } else {
                    session.cwd.clone()
                };
                let meta = format!(
                    "{} lines:{} tokens:{}",
                    short(&cwd, 32),
                    session.lines,
                    compact_number(session.total_tokens)
                );
                ListItem::new(vec![
                    Line::from(Span::styled(
                        title,
                        Style::default().add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(meta, Style::default().fg(Color::DarkGray))),
                ])
            })
            .collect()
    };

    let mut list_state = ListState::default();
    if !state.model.sessions.is_empty() {
        list_state.select(Some(state.model.selected_session));
    }
    let list = List::new(items)
        .block(Block::default().title("Sessions").borders(Borders::ALL))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_session_content(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let Some(session) = state.current_session() else {
        let empty = Paragraph::new(
            "No Codex sessions found.\nUse --root to point at a Codex home or run Codex first.",
        )
        .block(Block::default().title("Session").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
        frame.render_widget(empty, area);
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Percentage(46),
            Constraint::Percentage(54),
        ])
        .split(area);

    draw_stats(frame, chunks[0], session);
    draw_compactions(frame, chunks[1], session);
    draw_messages(frame, chunks[2], state);
    draw_detail(frame, chunks[3], state);
}

fn draw_stats(frame: &mut Frame<'_>, area: Rect, session: &TuiSession) {
    let session_path = session.path.display().to_string();
    let text = vec![
        Line::from(vec![
            Span::styled("Session ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(short(&session.session_id, 36)),
            Span::raw("  "),
            Span::styled("CWD ", Style::default().fg(Color::DarkGray)),
            Span::raw(short(&session.cwd, 60)),
        ]),
        Line::from(vec![Span::raw(format!(
            "messages:{} lines:{} compactions:{} tokens:{} context:{}",
            session.messages,
            session.lines,
            session.compactions,
            compact_number(session.total_tokens),
            compact_number(session.model_context_window)
        ))]),
        Line::from(Span::styled(
            short(&session_path, 120),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::default().title("Stats").borders(Borders::ALL)),
        area,
    );
}

fn draw_compactions(frame: &mut Frame<'_>, area: Rect, session: &TuiSession) {
    let text = if session.parsed.compaction_events.is_empty() {
        Text::from("No compaction events in this session.")
    } else {
        let lines = session
            .parsed
            .compaction_events
            .iter()
            .enumerate()
            .take(3)
            .map(|(index, event)| {
                let tokens = event
                    .token_usage
                    .as_ref()
                    .map(|usage| compact_number(usage.total_tokens))
                    .unwrap_or_else(|| "-".to_string());
                Line::from(format!(
                    "{}. line {} trigger:{} tokens:{} summary:{} chars",
                    index + 1,
                    event.line_number,
                    empty_dash(&event.trigger),
                    tokens,
                    event.summary_length()
                ))
            })
            .collect::<Vec<_>>();
        Text::from(lines)
    };

    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().title("Compactions").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_messages(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let compaction_lines = state
        .current_session()
        .map(|session| compaction_lines(&session.parsed.compaction_events))
        .unwrap_or_default();
    let rows = state.visible_messages().into_iter().map(|message| {
        let marker = if compaction_lines.contains(&message.line_number) {
            "*"
        } else {
            ""
        };
        Row::new(vec![
            Cell::from(format!("{}{}", marker, message.line_number)),
            Cell::from(short_time(&message.timestamp)),
            Cell::from(short(display_kind(message), 16)),
            Cell::from(short(&message.role, 12)),
            Cell::from(short(&message.content.replace('\n', " "), 72)),
        ])
    });
    let mut table_state = TableState::default();
    if !state.visible_messages().is_empty() {
        table_state.select(Some(state.selected_message));
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(17),
            Constraint::Length(13),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(["Line", "Time", "Type", "Role", "Preview"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().title("History").borders(Borders::ALL))
    .row_highlight_style(Style::default().fg(Color::Black).bg(Color::White));

    frame.render_stateful_widget(table, area, &mut table_state);
}

fn draw_detail(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let title = if state.show_summaries {
        "Summaries"
    } else {
        "Detail"
    };
    let paragraph = Paragraph::new(state.selected_detail_text())
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn session_row(parsed: ParsedSession) -> TuiSession {
    let stats = &parsed.stats;
    let metadata = &parsed.metadata;
    let session_id = if metadata.session_id.is_empty() {
        metadata
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        metadata.session_id.clone()
    };
    let started_at = if metadata.started_at.is_empty() {
        stats.first_timestamp.clone()
    } else {
        metadata.started_at.clone()
    };
    TuiSession {
        path: metadata.path.clone(),
        session_id,
        cwd: metadata.cwd.clone(),
        started_at,
        last_timestamp: stats.last_timestamp.clone(),
        lines: stats.line_count,
        messages: stats.message_count,
        compactions: parsed.compaction_events.len(),
        total_tokens: stats.total_tokens,
        model_context_window: stats.model_context_window,
        parsed,
    }
}

fn compaction_lines(events: &[CompactionEvent]) -> HashSet<usize> {
    let mut lines = HashSet::new();
    for event in events {
        lines.insert(event.line_number);
        if let Some(boundary_line) = event.boundary_line_number {
            lines.insert(boundary_line);
        }
    }
    lines
}

fn display_kind(message: &ParsedMessage) -> &str {
    if !message.kind.is_empty() {
        &message.kind
    } else if !message.record_type.is_empty() {
        &message.record_type
    } else {
        "message"
    }
}

fn move_index(current: usize, delta: isize, count: usize) -> usize {
    let next = current as isize + delta;
    next.clamp(0, count.saturating_sub(1) as isize) as usize
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

fn short_time(value: &str) -> String {
    if value.len() >= 19 {
        value[11..19].to_string()
    } else {
        short(value, 10)
    }
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

fn empty_dash(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}
