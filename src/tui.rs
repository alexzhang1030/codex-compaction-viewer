use crate::parser::{
    discover_sessions, parse_jsonl, CompactionEvent, ParsedMessage, ParsedSession,
};
use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
        Wrap,
    },
    Frame, Terminal,
};
use std::{
    collections::HashSet,
    io::{self, Write},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiFocus {
    History,
    Detail,
    SessionSearch,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TuiDisplayMode {
    #[default]
    Tidy,
    Verbose,
}

impl TuiDisplayMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tidy => "tidy",
            Self::Verbose => "verbose",
        }
    }

    fn toggled(self) -> Self {
        match self {
            Self::Tidy => Self::Verbose,
            Self::Verbose => Self::Tidy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiSelectionBlock {
    Sessions,
    Stats,
    Compactions,
    History,
    Detail,
    RawPopup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionPoint {
    block: TuiSelectionBlock,
    row: usize,
    column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TuiSelection {
    anchor: SelectionPoint,
    cursor: SelectionPoint,
}

#[derive(Debug, Clone)]
pub struct TuiState {
    pub model: TuiModel,
    pub selected_message: usize,
    pub show_summaries: bool,
    pub display_mode: TuiDisplayMode,
    show_raw_popup: bool,
    session_search: String,
    filter_compacted_sessions: bool,
    focus: TuiFocus,
    detail_scroll: u16,
    raw_popup_scroll: u16,
    mouse_capture_enabled: bool,
    selection_anchor: Option<SelectionPoint>,
    selection: Option<TuiSelection>,
}

#[derive(Debug, Clone, Copy)]
struct TuiLayout {
    title: Rect,
    sessions: Rect,
    stats: Rect,
    compactions: Rect,
    history: Rect,
    detail: Rect,
    footer: Rect,
    popup: Rect,
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
    display_mode: TuiDisplayMode,
    raw_bodies_enabled: bool,
    mouse_capture_enabled: bool,
) -> Result<()> {
    let model = build_tui_model(root, include_archived, initial_file)?;
    let mut state = TuiState::with_terminal_options(
        model,
        display_mode,
        raw_bodies_enabled,
        mouse_capture_enabled,
    );

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    if state.mouse_capture_enabled {
        execute!(stdout, EnableMouseCapture)?;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_terminal(&mut terminal, &mut state);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

impl TuiState {
    pub fn new(model: TuiModel) -> Self {
        Self::with_display_mode(model, TuiDisplayMode::default())
    }

    pub fn with_display_mode(model: TuiModel, display_mode: TuiDisplayMode) -> Self {
        Self::with_options(model, display_mode, false)
    }

    pub fn with_options(
        model: TuiModel,
        display_mode: TuiDisplayMode,
        raw_bodies_enabled: bool,
    ) -> Self {
        Self::with_terminal_options(model, display_mode, raw_bodies_enabled, true)
    }

    pub fn with_terminal_options(
        model: TuiModel,
        display_mode: TuiDisplayMode,
        raw_bodies_enabled: bool,
        mouse_capture_enabled: bool,
    ) -> Self {
        Self {
            selected_message: 0,
            model,
            show_summaries: false,
            display_mode,
            show_raw_popup: raw_bodies_enabled,
            session_search: String::new(),
            filter_compacted_sessions: false,
            focus: TuiFocus::History,
            detail_scroll: 0,
            raw_popup_scroll: 0,
            mouse_capture_enabled,
            selection_anchor: None,
            selection: None,
        }
    }

    pub fn set_session_search(&mut self, query: impl Into<String>) {
        let previous_session = self.model.selected_session;
        self.session_search = query.into();
        self.ensure_visible_session_selected();
        if self.model.selected_session != previous_session {
            self.selected_message = 0;
            self.show_summaries = false;
        }
        self.detail_scroll = 0;
        self.clear_selection();
    }

    pub fn session_search(&self) -> &str {
        &self.session_search
    }

    pub fn visible_session_ids(&self) -> Vec<&str> {
        self.visible_session_indices()
            .into_iter()
            .filter_map(|index| self.model.sessions.get(index))
            .map(|session| session.session_id.as_str())
            .collect()
    }

    pub fn current_session_id(&self) -> Option<&str> {
        self.current_session()
            .map(|session| session.session_id.as_str())
    }

    pub fn compaction_session_filter_enabled(&self) -> bool {
        self.filter_compacted_sessions
    }

    pub fn focus(&self) -> TuiFocus {
        self.focus
    }

    pub fn detail_scroll(&self) -> u16 {
        self.detail_scroll
    }

    pub fn raw_popup_visible(&self) -> bool {
        self.show_raw_popup
    }

    pub fn raw_popup_scroll(&self) -> u16 {
        self.raw_popup_scroll
    }

    pub fn display_mode(&self) -> TuiDisplayMode {
        self.display_mode
    }

    pub fn mouse_capture_enabled(&self) -> bool {
        self.mouse_capture_enabled
    }

    pub fn selection_block(&self) -> Option<TuiSelectionBlock> {
        self.selection.map(|selection| selection.anchor.block)
    }

    pub fn selected_text(&self) -> Option<String> {
        let selection = self.normalized_selection()?;
        let text = self.selection_block_text(selection.0.block);
        selected_text_from_range(&text, selection)
    }

    fn normalized_selection(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let selection = self.selection?;
        if selection.anchor.block != selection.cursor.block {
            return None;
        }
        let (start, end) = if (selection.anchor.row, selection.anchor.column)
            <= (selection.cursor.row, selection.cursor.column)
        {
            (selection.anchor, selection.cursor)
        } else {
            (selection.cursor, selection.anchor)
        };
        if start.row == end.row && start.column == end.column {
            None
        } else {
            Some((start, end))
        }
    }

    fn begin_selection(&mut self, point: SelectionPoint) {
        self.selection_anchor = Some(point);
        self.selection = None;
    }

    fn update_selection(&mut self, point: SelectionPoint) {
        let Some(anchor) = self.selection_anchor else {
            return;
        };
        if anchor.block != point.block {
            return;
        }
        self.selection = Some(TuiSelection {
            anchor,
            cursor: point,
        });
    }

    fn finish_selection(&mut self, point: SelectionPoint) {
        self.update_selection(point);
        self.selection_anchor = None;
        if self.selected_text().is_none() {
            self.selection = None;
        }
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection = None;
    }

    fn selection_block_text(&self, block: TuiSelectionBlock) -> String {
        match block {
            TuiSelectionBlock::Sessions => self.sessions_text(),
            TuiSelectionBlock::Stats => self.stats_text(),
            TuiSelectionBlock::Compactions => self.compactions_text(),
            TuiSelectionBlock::History => self.history_text(),
            TuiSelectionBlock::Detail => self.selected_detail_text(),
            TuiSelectionBlock::RawPopup => self.raw_popup_text(),
        }
    }

    pub fn footer_help_text(&self) -> String {
        let compacted_filter = if self.filter_compacted_sessions {
            "compacted:on"
        } else {
            "compacted:off"
        };
        let mouse_capture = if self.mouse_capture_enabled {
            "mouse:on"
        } else {
            "mouse:off"
        };
        let focus = match self.focus {
            TuiFocus::History => "history",
            TuiFocus::Detail => "detail",
            TuiFocus::SessionSearch => "search",
        };
        let selection = self
            .selected_text()
            .map(|text| format!(" | selected:{} chars y copy", text.chars().count()))
            .unwrap_or_default();
        format!(
            "q quit | / search | r raw | m {mouse_capture} | drag select | g {compacted_filter} | v mode:{} | Enter detail | j/k {focus} | h/l sessions | c/C compactions | s summaries{selection}",
            self.display_mode.as_str()
        )
    }

    pub fn selected_message_line(&self) -> Option<usize> {
        self.visible_messages()
            .get(self.selected_message)
            .map(|message| message.line_number)
    }

    pub fn raw_popup_text(&self) -> String {
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

        if !message.request_body.is_empty() {
            output.push_str("\nREQUEST BODY\n");
            output.push_str(&format_json_if_possible(&message.request_body));
            output.push('\n');
        }
        if !message.response_body.is_empty() {
            output.push_str("\nRESPONSE BODY\n");
            output.push_str(&format_json_if_possible(&message.response_body));
            output.push('\n');
        }
        if !message.raw_payload.is_empty() {
            output.push_str("\nRAW PAYLOAD\n");
            output.push_str(&message.raw_payload);
            output.push('\n');
        }
        if output.lines().count() <= 4 {
            output.push_str("\nNo raw request/response body is available for this row.");
        }

        if output.chars().count() > 40_000 {
            format!(
                "{}...\n\n(truncated)",
                output.chars().take(40_000).collect::<String>()
            )
        } else {
            output
        }
    }

    fn sessions_text(&self) -> String {
        let visible_indices = self.visible_session_indices();
        if self.model.sessions.is_empty() {
            return "No Codex sessions found".to_string();
        }
        if visible_indices.is_empty() {
            return "No sessions match the current search".to_string();
        }

        visible_indices
            .iter()
            .filter_map(|index| self.model.sessions.get(*index))
            .flat_map(|session| {
                let cwd = if session.cwd.is_empty() {
                    session.path.display().to_string()
                } else {
                    session.cwd.clone()
                };
                [
                    format!(
                        "{}  compactions:{}",
                        short(&session.session_id, 18),
                        session.compactions
                    ),
                    format!(
                        "{} lines:{} tokens:{}",
                        short(&cwd, 32),
                        session.lines,
                        compact_number(session.total_tokens)
                    ),
                ]
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn stats_text(&self) -> String {
        let Some(session) = self.current_session() else {
            return "No session selected.".to_string();
        };
        stats_lines(session).join("\n")
    }

    fn compactions_text(&self) -> String {
        let Some(session) = self.current_session() else {
            return "No session selected.".to_string();
        };
        compaction_lines_text(session).join("\n")
    }

    fn history_text(&self) -> String {
        let compaction_lines = self
            .current_session()
            .map(|session| compaction_lines(&session.parsed.compaction_events))
            .unwrap_or_default();
        let mut lines =
            vec!["Line    Time       Type              Role          Preview".to_string()];
        lines.extend(self.visible_messages().into_iter().map(|message| {
            let marker = if compaction_lines.contains(&message.line_number) {
                "*"
            } else {
                ""
            };
            format!(
                "{:<7} {:<10} {:<17} {:<13} {}",
                format!("{}{}", marker, message.line_number),
                short_time(&message.timestamp),
                short(display_kind(message), 16),
                short(&message.role, 12),
                short(&message.content.replace('\n', " "), 72)
            )
        }));
        lines.join("\n")
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
        self.detail_scroll = 0;
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
        self.detail_scroll = 0;
    }

    fn current_session(&self) -> Option<&TuiSession> {
        self.model
            .sessions
            .get(self.model.selected_session)
            .filter(|session| self.session_matches(session))
    }

    fn visible_messages(&self) -> Vec<&ParsedMessage> {
        self.current_session()
            .map(|session| {
                let compact_lines = compaction_lines(&session.parsed.compaction_events);
                session
                    .parsed
                    .messages
                    .iter()
                    .filter(|message| match self.display_mode {
                        TuiDisplayMode::Tidy => is_tidy_message(message, &compact_lines),
                        TuiDisplayMode::Verbose => true,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn visible_session_indices(&self) -> Vec<usize> {
        self.model
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| self.session_matches(session).then_some(index))
            .collect()
    }

    fn session_matches(&self, session: &TuiSession) -> bool {
        if self.filter_compacted_sessions && session.compactions == 0 {
            return false;
        }

        self.session_search
            .split_whitespace()
            .all(|term| session_matches_term(session, term))
    }

    fn ensure_visible_session_selected(&mut self) {
        let indices = self.visible_session_indices();
        if indices.is_empty() {
            self.selected_message = 0;
            self.detail_scroll = 0;
            return;
        }

        if !indices.contains(&self.model.selected_session) {
            self.model.selected_session = indices[0];
            self.selected_message = 0;
            self.show_summaries = false;
            self.detail_scroll = 0;
        }
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
        self.detail_scroll = 0;
        self.clear_selection();
    }

    fn page_message(&mut self, delta: isize) {
        self.move_message(delta * 10);
    }

    fn scroll_detail(&mut self, delta: isize) {
        if delta.is_negative() {
            self.detail_scroll = self
                .detail_scroll
                .saturating_sub(delta.unsigned_abs() as u16);
        } else {
            self.detail_scroll = self.detail_scroll.saturating_add(delta as u16);
        }
    }

    fn scroll_raw_popup(&mut self, delta: isize) {
        if delta.is_negative() {
            self.raw_popup_scroll = self
                .raw_popup_scroll
                .saturating_sub(delta.unsigned_abs() as u16);
        } else {
            self.raw_popup_scroll = self.raw_popup_scroll.saturating_add(delta as u16);
        }
    }

    fn toggle_raw_popup(&mut self) {
        self.show_raw_popup = !self.show_raw_popup;
        self.raw_popup_scroll = 0;
        self.clear_selection();
    }

    fn close_raw_popup(&mut self) {
        self.show_raw_popup = false;
        self.raw_popup_scroll = 0;
        self.clear_selection();
    }

    fn toggle_mouse_capture(&mut self) {
        self.mouse_capture_enabled = !self.mouse_capture_enabled;
    }

    fn move_session(&mut self, delta: isize) {
        let indices = self.visible_session_indices();
        if indices.is_empty() {
            self.model.selected_session = 0;
            self.selected_message = 0;
            self.detail_scroll = 0;
            return;
        }
        let current_position = indices
            .iter()
            .position(|index| *index == self.model.selected_session)
            .unwrap_or(0);
        let next_position = move_index(current_position, delta, indices.len());
        self.model.selected_session = indices[next_position];
        self.selected_message = 0;
        self.show_summaries = false;
        self.show_raw_popup = false;
        self.detail_scroll = 0;
        self.raw_popup_scroll = 0;
        self.clear_selection();
    }

    fn toggle_compaction_session_filter(&mut self) {
        self.filter_compacted_sessions = !self.filter_compacted_sessions;
        self.ensure_visible_session_selected();
        self.detail_scroll = 0;
        self.clear_selection();
    }

    fn toggle_display_mode(&mut self) {
        self.display_mode = self.display_mode.toggled();
        self.selected_message = 0;
        self.show_summaries = false;
        self.detail_scroll = 0;
        self.clear_selection();
    }

    fn push_session_search_char(&mut self, ch: char) {
        self.session_search.push(ch);
        self.ensure_visible_session_selected();
        self.detail_scroll = 0;
        self.clear_selection();
    }

    fn pop_session_search_char(&mut self) {
        self.session_search.pop();
        self.ensure_visible_session_selected();
        self.detail_scroll = 0;
        self.clear_selection();
    }
}

fn run_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
) -> Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, state))?;

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => {
                    if should_copy_selection(state, key) {
                        copy_selection_to_clipboard(terminal, state)?;
                        continue;
                    }
                    let previous_mouse_capture = state.mouse_capture_enabled();
                    if handle_key(state, key) {
                        return Ok(());
                    }
                    sync_mouse_capture(
                        terminal,
                        previous_mouse_capture,
                        state.mouse_capture_enabled(),
                    )?;
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    let area = Rect::new(0, 0, size.width, size.height);
                    handle_mouse(state, mouse, area);
                }
                _ => {}
            }
        }
    }
}

fn should_copy_selection(state: &TuiState, key: KeyEvent) -> bool {
    state.focus != TuiFocus::SessionSearch
        && key.code == KeyCode::Char('y')
        && key.modifiers.is_empty()
        && state.selected_text().is_some()
}

fn copy_selection_to_clipboard(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &TuiState,
) -> Result<()> {
    let Some(text) = state.selected_text() else {
        return Ok(());
    };
    write!(terminal.backend_mut(), "{}", osc52_copy_sequence(&text))?;
    terminal.backend_mut().flush()?;
    Ok(())
}

fn sync_mouse_capture(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    previous: bool,
    current: bool,
) -> Result<()> {
    if previous == current {
        return Ok(());
    }
    if current {
        execute!(terminal.backend_mut(), EnableMouseCapture)?;
    } else {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    Ok(())
}

pub fn handle_key(state: &mut TuiState, key: KeyEvent) -> bool {
    if state.show_raw_popup {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => state.close_raw_popup(),
            KeyCode::Char('j') | KeyCode::Down => state.scroll_raw_popup(1),
            KeyCode::Char('k') | KeyCode::Up => state.scroll_raw_popup(-1),
            KeyCode::PageDown => state.scroll_raw_popup(10),
            KeyCode::PageUp => state.scroll_raw_popup(-10),
            KeyCode::Char('m') => state.toggle_mouse_capture(),
            _ => {}
        }
        return false;
    }

    if state.focus == TuiFocus::SessionSearch {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => state.focus = TuiFocus::History,
            KeyCode::Backspace => state.pop_session_search_char(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.set_session_search("");
            }
            KeyCode::Char(ch)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                state.push_session_search_char(ch);
            }
            _ => {}
        }
        return false;
    }

    if key.code == KeyCode::Char('q') {
        return true;
    }

    if state.focus == TuiFocus::Detail {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => state.focus = TuiFocus::History,
            KeyCode::Char('j') | KeyCode::Down => state.scroll_detail(1),
            KeyCode::Char('k') | KeyCode::Up => state.scroll_detail(-1),
            KeyCode::PageDown => state.scroll_detail(10),
            KeyCode::PageUp => state.scroll_detail(-10),
            KeyCode::Char('m') => state.toggle_mouse_capture(),
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Esc => return true,
        KeyCode::Char('/') => state.focus = TuiFocus::SessionSearch,
        KeyCode::Enter => {
            state.focus = TuiFocus::Detail;
            state.detail_scroll = 0;
        }
        KeyCode::Char('g') => state.toggle_compaction_session_filter(),
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
        KeyCode::Char('s') => {
            state.show_summaries = !state.show_summaries;
            state.detail_scroll = 0;
        }
        KeyCode::Char('v') => state.toggle_display_mode(),
        KeyCode::Char('m') => state.toggle_mouse_capture(),
        KeyCode::Char('r') => state.toggle_raw_popup(),
        _ => {}
    }
    false
}

pub fn handle_mouse(state: &mut TuiState, mouse: MouseEvent, area: Rect) {
    if !state.mouse_capture_enabled {
        return;
    }

    let layout = tui_layout(area);
    if state.show_raw_popup {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(point) = selection_point_at(state, layout, mouse.column, mouse.row) {
                    state.begin_selection(point);
                } else {
                    state.clear_selection();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(point) = selection_drag_point(state, layout, mouse.column, mouse.row) {
                    state.update_selection(point);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(point) = selection_drag_point(state, layout, mouse.column, mouse.row) {
                    state.finish_selection(point);
                } else {
                    state.clear_selection();
                }
            }
            MouseEventKind::ScrollDown => state.scroll_raw_popup(3),
            MouseEventKind::ScrollUp => state.scroll_raw_popup(-3),
            MouseEventKind::Down(MouseButton::Right) => {
                state.clear_selection();
                state.close_raw_popup();
            }
            _ => {}
        }
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(point) = selection_point_at(state, layout, mouse.column, mouse.row) {
                state.begin_selection(point);
            } else {
                state.clear_selection();
            }
            if contains(layout.sessions, mouse.column, mouse.row) {
                select_session_at(state, layout.sessions, mouse.row);
                state.focus = TuiFocus::History;
            } else if contains(layout.history, mouse.column, mouse.row) {
                select_message_at(state, layout.history, mouse.row);
                state.focus = TuiFocus::History;
            } else if contains(layout.detail, mouse.column, mouse.row) {
                state.focus = TuiFocus::Detail;
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(point) = selection_drag_point(state, layout, mouse.column, mouse.row) {
                state.update_selection(point);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some(point) = selection_drag_point(state, layout, mouse.column, mouse.row) {
                state.finish_selection(point);
            } else {
                state.clear_selection();
            }
        }
        MouseEventKind::ScrollDown => {
            if contains(layout.detail, mouse.column, mouse.row) || state.focus == TuiFocus::Detail {
                state.focus = TuiFocus::Detail;
                state.scroll_detail(3);
            } else if contains(layout.sessions, mouse.column, mouse.row) {
                state.move_session(1);
            } else {
                state.move_message(3);
            }
        }
        MouseEventKind::ScrollUp => {
            if contains(layout.detail, mouse.column, mouse.row) || state.focus == TuiFocus::Detail {
                state.focus = TuiFocus::Detail;
                state.scroll_detail(-3);
            } else if contains(layout.sessions, mouse.column, mouse.row) {
                state.move_session(-1);
            } else {
                state.move_message(-3);
            }
        }
        _ => {}
    }
}

fn selection_point_at(
    state: &TuiState,
    layout: TuiLayout,
    column: u16,
    row: u16,
) -> Option<SelectionPoint> {
    let block = selection_block_at(state, layout, column, row)?;
    Some(selection_point_for_block(state, layout, block, column, row))
}

fn selection_drag_point(
    state: &TuiState,
    layout: TuiLayout,
    column: u16,
    row: u16,
) -> Option<SelectionPoint> {
    let block = state.selection_anchor?.block;
    Some(selection_point_for_block(state, layout, block, column, row))
}

fn selection_block_at(
    state: &TuiState,
    layout: TuiLayout,
    column: u16,
    row: u16,
) -> Option<TuiSelectionBlock> {
    if state.show_raw_popup {
        return contains(layout.popup, column, row).then_some(TuiSelectionBlock::RawPopup);
    }

    [
        (TuiSelectionBlock::Sessions, layout.sessions),
        (TuiSelectionBlock::Stats, layout.stats),
        (TuiSelectionBlock::Compactions, layout.compactions),
        (TuiSelectionBlock::History, layout.history),
        (TuiSelectionBlock::Detail, layout.detail),
    ]
    .into_iter()
    .find_map(|(block, area)| contains(area, column, row).then_some(block))
}

fn selection_point_for_block(
    state: &TuiState,
    layout: TuiLayout,
    block: TuiSelectionBlock,
    column: u16,
    row: u16,
) -> SelectionPoint {
    let area = selection_area(layout, block);
    let inner = inner_area(area);
    let relative_column = if inner.width == 0 || column <= inner.x {
        0
    } else {
        let right = inner.x.saturating_add(inner.width);
        column.min(right).saturating_sub(inner.x) as usize
    };
    let visual_row = if inner.height == 0 || row <= inner.y {
        0
    } else {
        let bottom = inner.y.saturating_add(inner.height.saturating_sub(1));
        row.min(bottom).saturating_sub(inner.y) as usize
    };
    SelectionPoint {
        block,
        row: visual_row + selection_scroll(state, block) as usize,
        column: relative_column,
    }
}

fn selection_area(layout: TuiLayout, block: TuiSelectionBlock) -> Rect {
    match block {
        TuiSelectionBlock::Sessions => layout.sessions,
        TuiSelectionBlock::Stats => layout.stats,
        TuiSelectionBlock::Compactions => layout.compactions,
        TuiSelectionBlock::History => layout.history,
        TuiSelectionBlock::Detail => layout.detail,
        TuiSelectionBlock::RawPopup => layout.popup,
    }
}

fn selection_scroll(state: &TuiState, block: TuiSelectionBlock) -> u16 {
    match block {
        TuiSelectionBlock::Detail => state.detail_scroll,
        TuiSelectionBlock::RawPopup => state.raw_popup_scroll,
        _ => 0,
    }
}

fn inner_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

fn selected_text_from_range(
    text: &str,
    (start, end): (SelectionPoint, SelectionPoint),
) -> Option<String> {
    let lines = text.split('\n').collect::<Vec<_>>();
    if start.row >= lines.len() {
        return None;
    }

    let end_row = end.row.min(lines.len().saturating_sub(1));
    let mut output = Vec::new();
    for (row, line) in lines.iter().enumerate().take(end_row + 1).skip(start.row) {
        let line_len = line.chars().count();
        let start_column = if row == start.row {
            start.column.min(line_len)
        } else {
            0
        };
        let end_column = if row == end_row {
            end.column.min(line_len)
        } else {
            line_len
        };
        if start_column <= end_column {
            output.push(slice_chars(line, start_column, end_column));
        }
    }

    let selected = output.join("\n");
    (!selected.is_empty()).then_some(selected)
}

fn osc52_copy_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        let combined = ((first as u32) << 16) | ((second as u32) << 8) | third as u32;

        output.push(TABLE[((combined >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((combined >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[((combined >> 6) & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(combined & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn selectable_text(state: &TuiState, block: TuiSelectionBlock, text: String) -> Text<'static> {
    let Some((start, end)) = state.normalized_selection() else {
        return Text::from(text);
    };
    if start.block != block {
        return Text::from(text);
    }

    let selected_style = Style::default().fg(Color::Black).bg(Color::Yellow);
    let lines = text
        .split('\n')
        .enumerate()
        .map(|(row, line)| {
            if row < start.row || row > end.row {
                return Line::from(line.to_string());
            }
            let line_len = line.chars().count();
            let start_column = if row == start.row {
                start.column.min(line_len)
            } else {
                0
            };
            let end_column = if row == end.row {
                end.column.min(line_len)
            } else {
                line_len
            };

            if start_column >= end_column {
                return Line::from(line.to_string());
            }

            Line::from(vec![
                Span::raw(slice_chars(line, 0, start_column)),
                Span::styled(slice_chars(line, start_column, end_column), selected_style),
                Span::raw(slice_chars(line, end_column, line_len)),
            ])
        })
        .collect::<Vec<_>>();

    Text::from(lines)
}

fn draw(frame: &mut Frame<'_>, state: &TuiState) {
    let layout = tui_layout(frame.area());

    draw_title(frame, layout.title);
    draw_footer(frame, layout.footer, state);
    draw_sessions(frame, layout.sessions, state);
    draw_session_content(frame, right_content_area(frame.area()), state);
    if state.show_raw_popup {
        draw_raw_popup(frame, layout.popup, state);
    }
}

fn tui_layout(area: Rect) -> TuiLayout {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(38), Constraint::Min(40)])
        .split(root[1]);

    let content = session_content_layout(body[1]);

    TuiLayout {
        title: root[0],
        sessions: body[0],
        stats: content[0],
        compactions: content[1],
        history: content[2],
        detail: content[3],
        footer: root[2],
        popup: centered_rect(82, 76, area),
    }
}

fn right_content_area(area: Rect) -> Rect {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(38), Constraint::Min(40)])
        .split(root[1])[1]
}

fn session_content_layout(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Percentage(46),
            Constraint::Percentage(54),
        ])
        .split(area)
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn draw_title(frame: &mut Frame<'_>, area: Rect) {
    let title = Paragraph::new(format!(
        "{} {} - Codex Compaction Viewer",
        crate::APP_NAME,
        crate::APP_VERSION
    ))
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(title, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(
        Paragraph::new(state.footer_help_text()).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_sessions(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let visible_indices = state.visible_session_indices();
    let items = if state.model.sessions.is_empty() {
        vec![ListItem::new("No Codex sessions found")]
    } else if visible_indices.is_empty() {
        vec![ListItem::new("No sessions match the current search")]
    } else {
        visible_indices
            .iter()
            .filter_map(|index| state.model.sessions.get(*index))
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
    if !visible_indices.is_empty() {
        list_state.select(
            visible_indices
                .iter()
                .position(|index| *index == state.model.selected_session),
        );
    }
    let list = List::new(items)
        .block(focused_block(
            sessions_title(state),
            state.focus == TuiFocus::SessionSearch,
        ))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_session_content(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let Some(_session) = state.current_session() else {
        let message = if state.model.sessions.is_empty() {
            "No Codex sessions found.\nUse --root to point at a Codex home or run Codex first."
        } else {
            "No sessions match the current search or tag filter."
        };
        let empty = Paragraph::new(message)
            .block(Block::default().title("Session").borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        frame.render_widget(empty, area);
        return;
    };

    let chunks = session_content_layout(area);

    draw_stats(frame, chunks[0], state);
    draw_compactions(frame, chunks[1], state);
    draw_messages(frame, chunks[2], state);
    draw_detail(frame, chunks[3], state);
}

fn draw_stats(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(
        Paragraph::new(selectable_text(
            state,
            TuiSelectionBlock::Stats,
            state.stats_text(),
        ))
        .block(Block::default().title("Stats").borders(Borders::ALL)),
        area,
    );
}

fn draw_compactions(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(
        Paragraph::new(selectable_text(
            state,
            TuiSelectionBlock::Compactions,
            state.compactions_text(),
        ))
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
    .block(focused_block(
        "History".to_string(),
        state.focus == TuiFocus::History,
    ))
    .row_highlight_style(Style::default().fg(Color::Black).bg(Color::White));

    frame.render_stateful_widget(table, area, &mut table_state);
}

fn draw_detail(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let title = if state.show_summaries {
        "Summaries"
    } else {
        "Detail"
    };
    let paragraph = Paragraph::new(selectable_text(
        state,
        TuiSelectionBlock::Detail,
        state.selected_detail_text(),
    ))
    .block(focused_block(
        title.to_string(),
        state.focus == TuiFocus::Detail,
    ))
    .scroll((state.detail_scroll, 0))
    .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_raw_popup(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Clear, area);
    let paragraph = Paragraph::new(selectable_text(
        state,
        TuiSelectionBlock::RawPopup,
        state.raw_popup_text(),
    ))
    .block(focused_block("Raw Body  Esc/q close".to_string(), true))
    .scroll((state.raw_popup_scroll, 0))
    .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn stats_lines(session: &TuiSession) -> Vec<String> {
    let session_path = session.path.display().to_string();
    vec![
        format!(
            "Session {}  CWD {}",
            short(&session.session_id, 36),
            short(&session.cwd, 60)
        ),
        format!(
            "messages:{} lines:{} compactions:{} tokens:{} context:{}",
            session.messages,
            session.lines,
            session.compactions,
            compact_number(session.total_tokens),
            compact_number(session.model_context_window)
        ),
        short(&session_path, 120),
    ]
}

fn compaction_lines_text(session: &TuiSession) -> Vec<String> {
    if session.parsed.compaction_events.is_empty() {
        return vec!["No compaction events in this session.".to_string()];
    }

    session
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
            format!(
                "{}. line {} trigger:{} tokens:{} summary:{} chars",
                index + 1,
                event.line_number,
                empty_dash(&event.trigger),
                tokens,
                event.summary_length()
            )
        })
        .collect()
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

fn focused_block(title: String, focused: bool) -> Block<'static> {
    let block = Block::default().title(title).borders(Borders::ALL);
    if focused {
        block
            .border_style(Style::default().fg(Color::Cyan))
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
    } else {
        block
    }
}

fn sessions_title(state: &TuiState) -> String {
    let mut parts = vec!["Sessions".to_string()];
    if !state.session_search.is_empty() {
        parts.push(format!("/{}", short(&state.session_search, 24)));
    }
    if state.filter_compacted_sessions {
        parts.push("tag:compaction".to_string());
    }
    parts.join(" ")
}

fn select_session_at(state: &mut TuiState, area: Rect, row: u16) {
    let visible_indices = state.visible_session_indices();
    if visible_indices.is_empty() {
        return;
    }
    let row = row.saturating_sub(area.y + 1) as usize;
    let index = row / 2;
    if let Some(session_index) = visible_indices.get(index).copied() {
        state.model.selected_session = session_index;
        state.selected_message = 0;
        state.show_summaries = false;
        state.detail_scroll = 0;
    }
}

fn select_message_at(state: &mut TuiState, area: Rect, row: u16) {
    let count = state.visible_messages().len();
    if count == 0 {
        state.selected_message = 0;
        return;
    }
    let index = row.saturating_sub(area.y + 2) as usize;
    if index < count {
        state.selected_message = index;
        state.show_summaries = false;
        state.detail_scroll = 0;
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn session_matches_term(session: &TuiSession, term: &str) -> bool {
    let term = term.trim().to_ascii_lowercase();
    if term.is_empty() {
        return true;
    }

    if let Some((scope, value)) = term.split_once(':') {
        return match scope {
            "tag" | "has" => is_compaction_tag(value) && session.compactions > 0,
            "project" | "cwd" => {
                contains_lower(&session.cwd, value)
                    || contains_lower(&session.path.display().to_string(), value)
            }
            "session" | "id" => contains_lower(&session.session_id, value),
            _ => session_contains_text(session, &term),
        };
    }

    session_contains_text(session, &term)
}

fn session_contains_text(session: &TuiSession, needle: &str) -> bool {
    contains_lower(&session.session_id, needle)
        || contains_lower(&session.cwd, needle)
        || contains_lower(&session.path.display().to_string(), needle)
}

fn contains_lower(value: &str, needle: &str) -> bool {
    value.to_ascii_lowercase().contains(needle)
}

fn is_compaction_tag(value: &str) -> bool {
    matches!(
        value,
        "compaction" | "compactions" | "compact" | "compacted"
    )
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

fn is_tidy_message(message: &ParsedMessage, compact_lines: &HashSet<usize>) -> bool {
    if compact_lines.contains(&message.line_number) {
        return true;
    }

    match message.role.as_str() {
        "user" => true,
        "assistant" => matches!(
            message.kind.as_str(),
            "message" | "agent_message" | "assistant"
        ),
        "tool_call" | "tool" => true,
        _ => false,
    }
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

fn slice_chars(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
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

fn format_json_if_possible(value: &str) -> String {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

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
}
