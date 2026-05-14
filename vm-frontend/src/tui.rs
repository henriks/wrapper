use std::net::ToSocketAddrs;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use agentvm_frontend::payload_client::{
    PayloadClientError, PayloadEvent, PayloadRequest, PayloadSession, PayloadWriter,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect, Size};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use tui_term::widget::PseudoTerminal;

const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TuiLayout {
    pub(crate) terminal: Rect,
    pub(crate) status: Rect,
    pub(crate) guest_rows: u16,
    pub(crate) guest_cols: u16,
}

pub(crate) fn viewport_layout(area: Rect) -> TuiLayout {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let terminal = chunks[0];
    let status = chunks[1];
    TuiLayout {
        terminal,
        status,
        guest_rows: terminal.height.max(1),
        guest_cols: terminal.width.max(1),
    }
}

pub(crate) fn initial_guest_size(terminal_size: Size) -> (u16, u16) {
    let layout = viewport_layout(Rect::new(0, 0, terminal_size.width, terminal_size.height));
    (layout.guest_rows, layout.guest_cols)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartupSelection {
    pub(crate) enable_codex: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupDialogResult {
    Accepted(StartupSelection),
    Cancelled,
    Redraw,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartupDialog {
    enable_codex: bool,
}

impl StartupDialog {
    fn new() -> Self {
        Self { enable_codex: true }
    }

    fn handle_key(&mut self, key: KeyEvent) -> StartupDialogResult {
        if key.kind == KeyEventKind::Release {
            return StartupDialogResult::Ignored;
        }
        match key.code {
            KeyCode::Enter => StartupDialogResult::Accepted(StartupSelection {
                enable_codex: self.enable_codex,
            }),
            KeyCode::Esc => StartupDialogResult::Cancelled,
            KeyCode::Char('y' | 'Y') => {
                self.enable_codex = true;
                StartupDialogResult::Accepted(StartupSelection { enable_codex: true })
            }
            KeyCode::Char('n' | 'N') => {
                self.enable_codex = false;
                StartupDialogResult::Accepted(StartupSelection {
                    enable_codex: false,
                })
            }
            KeyCode::Char(' ') => {
                self.enable_codex = !self.enable_codex;
                StartupDialogResult::Redraw
            }
            _ => StartupDialogResult::Ignored,
        }
    }

    fn render(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area(), 64, 11);
        let choice = if self.enable_codex { "yes" } else { "no" };
        let body = format!(
            "Initialize Codex for this sandbox?\n\nCodex state will be mounted read-write under the project guest home.\n\n[Y] yes  [N] no  [Space] toggle  [Enter] accept  [Esc] cancel\n\nCurrent: {choice}"
        );
        let paragraph = Paragraph::new(body)
            .block(
                Block::default()
                    .title(" Sandbox Setup ")
                    .borders(Borders::ALL),
            )
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(crate) fn run_startup_dialog() -> Result<StartupSelection, String> {
    let mut terminal = ratatui::try_init().map_err(|error| error.to_string())?;
    let _restore = RestoreTerminal;
    let mut dialog = StartupDialog::new();
    terminal
        .draw(|frame| dialog.render(frame))
        .map_err(|error| error.to_string())?;
    loop {
        if event::poll(INPUT_POLL_INTERVAL).map_err(|error| error.to_string())? {
            let event = event::read().map_err(|error| error.to_string())?;
            if let Event::Key(key) = event {
                match dialog.handle_key(key) {
                    StartupDialogResult::Accepted(selection) => return Ok(selection),
                    StartupDialogResult::Cancelled => {
                        return Err("startup dialog cancelled".to_string());
                    }
                    StartupDialogResult::Redraw => terminal
                        .draw(|frame| dialog.render(frame))
                        .map_err(|error| error.to_string())
                        .map(|_| ())?,
                    StartupDialogResult::Ignored => {}
                }
            }
        }
    }
}

pub(crate) struct GuestTerminalView {
    parser: vt100::Parser,
    status: StatusBar,
    prompt: Option<PromptState>,
    prefix_armed: bool,
    last_prompt_result: Option<PromptResult>,
}

impl GuestTerminalView {
    pub(crate) fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows.max(1), cols.max(1), 2000),
            status: StatusBar::new(rows.max(1), cols.max(1)),
            prompt: None,
            prefix_armed: false,
            last_prompt_result: None,
        }
    }

    pub(crate) fn process_output(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.status.phase = SessionPhase::Running;
    }

    pub(crate) fn set_guest_size(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
        self.status.guest_rows = rows.max(1);
        self.status.guest_cols = cols.max(1);
    }

    pub(crate) fn set_phase(&mut self, phase: SessionPhase) {
        self.status.phase = phase;
    }

    fn open_prompt(&mut self, question: impl Into<String>) {
        self.prompt = Some(PromptState::new(question));
        self.status.focus = FocusMode::WrapperPrompt;
        self.prefix_armed = false;
    }

    fn handle_wrapper_key(&mut self, key: KeyEvent) -> WrapperKeyOutcome {
        if self.prompt.is_some() {
            return self.handle_prompt_key(key);
        }
        if self.prefix_armed {
            self.prefix_armed = false;
            if key.kind != KeyEventKind::Release && key.code == KeyCode::Char('p') {
                self.open_prompt("wrapper prompt");
                return WrapperKeyOutcome::Redraw;
            }
            return WrapperKeyOutcome::Redraw;
        }
        match key_event_to_guest_input(key) {
            Some(GuestInput::Reserved) => {
                self.prefix_armed = true;
                WrapperKeyOutcome::Redraw
            }
            Some(input) => WrapperKeyOutcome::GuestInput(input),
            None => WrapperKeyOutcome::Ignored,
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> WrapperKeyOutcome {
        if key.kind == KeyEventKind::Release {
            return WrapperKeyOutcome::Ignored;
        }
        let Some(prompt) = self.prompt.as_mut() else {
            return WrapperKeyOutcome::Ignored;
        };
        match key.code {
            KeyCode::Enter => {
                let result = PromptResult::Accepted {
                    value: prompt.input.clone(),
                };
                self.prompt = None;
                self.status.focus = FocusMode::Guest;
                self.last_prompt_result = Some(result);
                WrapperKeyOutcome::PromptFinished
            }
            KeyCode::Esc => {
                self.prompt = None;
                self.status.focus = FocusMode::Guest;
                self.last_prompt_result = Some(PromptResult::Cancelled);
                WrapperKeyOutcome::PromptFinished
            }
            KeyCode::Backspace => {
                prompt.input.pop();
                WrapperKeyOutcome::Redraw
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.input.push(ch);
                WrapperKeyOutcome::Redraw
            }
            _ => WrapperKeyOutcome::Ignored,
        }
    }

    pub(crate) fn render(&self, frame: &mut Frame) {
        let layout = viewport_layout(frame.area());
        let terminal = PseudoTerminal::new(self.parser.screen())
            .style(Style::default().fg(Color::White).bg(Color::Black));
        frame.render_widget(terminal, layout.terminal);
        let status_text = self
            .prompt
            .as_ref()
            .map(|prompt| prompt.text(layout.status.width))
            .unwrap_or_else(|| self.status.text(layout.status.width));
        frame.render_widget(Paragraph::new(status_text), layout.status);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptState {
    question: String,
    input: String,
}

impl PromptState {
    fn new(question: impl Into<String>) -> Self {
        Self {
            question: question.into(),
            input: String::new(),
        }
    }

    fn text(&self, width: u16) -> String {
        truncate_status(format!("prompt | {}: {}", self.question, self.input), width)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptResult {
    Accepted { value: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionPhase {
    StartingPayload,
    Running,
    Exited(i32),
    Error(String),
}

impl SessionPhase {
    fn label(&self) -> String {
        match self {
            Self::StartingPayload => "starting payload".to_string(),
            Self::Running => "running".to_string(),
            Self::Exited(exit_code) => format!("exited {exit_code}"),
            Self::Error(message) => format!("error: {message}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusMode {
    Guest,
    WrapperPrompt,
}

impl FocusMode {
    fn label(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::WrapperPrompt => "prompt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusBar {
    phase: SessionPhase,
    focus: FocusMode,
    guest_rows: u16,
    guest_cols: u16,
}

impl StatusBar {
    fn new(guest_rows: u16, guest_cols: u16) -> Self {
        Self {
            phase: SessionPhase::StartingPayload,
            focus: FocusMode::Guest,
            guest_rows,
            guest_cols,
        }
    }

    fn text(&self, width: u16) -> String {
        let text = format!(
            "{} | focus {} | {}x{}",
            self.phase.label(),
            self.focus.label(),
            self.guest_cols,
            self.guest_rows
        );
        truncate_status(text, width)
    }
}

fn truncate_status(mut text: String, width: u16) -> String {
    let width = usize::from(width);
    if text.chars().count() <= width {
        return text;
    }
    if width == 0 {
        return String::new();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }
    let keep = width - 3;
    text = text.chars().take(keep).collect();
    text.push_str("...");
    text
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GuestInput {
    Bytes(Vec<u8>),
    Signal(i32),
    Reserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WrapperKeyOutcome {
    GuestInput(GuestInput),
    PromptFinished,
    Redraw,
    Ignored,
}

fn key_event_to_guest_input(key: KeyEvent) -> Option<GuestInput> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if ctrl && key.code == KeyCode::Char('\\') {
        return Some(GuestInput::Reserved);
    }
    if ctrl && matches!(key.code, KeyCode::Char('c' | 'C')) {
        return Some(GuestInput::Signal(libc::SIGINT));
    }

    let bytes = match key.code {
        KeyCode::Char(ch) if ctrl => ctrl_char_bytes(ch)?,
        KeyCode::Char(ch) if alt => {
            let mut bytes = vec![0x1b];
            let mut encoded = [0; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
            bytes
        }
        KeyCode::Char(ch) => {
            let mut encoded = [0; 4];
            ch.encode_utf8(&mut encoded).as_bytes().to_vec()
        }
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        _ => return None,
    };
    Some(GuestInput::Bytes(bytes))
}

fn ctrl_char_bytes(ch: char) -> Option<Vec<u8>> {
    let lower = ch.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        Some(vec![lower as u8 - b'a' + 1])
    } else {
        match ch {
            '[' => Some(vec![0x1b]),
            ']' => Some(vec![0x1d]),
            '^' => Some(vec![0x1e]),
            '_' => Some(vec![0x1f]),
            _ => None,
        }
    }
}

fn handle_guest_input(
    input: GuestInput,
    writer: &mut PayloadWriter,
) -> Result<(), PayloadClientError> {
    match input {
        GuestInput::Bytes(bytes) => writer.send_input(&bytes),
        GuestInput::Signal(signal) => writer.send_signal(signal),
        GuestInput::Reserved => Ok(()),
    }
}

struct RestoreTerminal;

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = ratatui::try_restore();
    }
}

pub(crate) fn run_payload_viewport(
    addr: impl ToSocketAddrs,
    request: &PayloadRequest,
) -> Result<i32, PayloadClientError> {
    let mut terminal = ratatui::try_init()?;
    let _restore = RestoreTerminal;
    let terminal_size = terminal.size()?;
    let (rows, cols) = initial_guest_size(terminal_size);
    let mut request = request.clone();
    request.rows = rows;
    request.cols = cols;
    let mut view = GuestTerminalView::new(rows, cols);
    terminal.draw(|frame| view.render(frame))?;

    let mut session = PayloadSession::connect(addr, &request)?;
    let mut writer = session.try_clone_writer()?;
    let (payload_tx, payload_rx) = mpsc::channel();
    thread::Builder::new()
        .name("agentvm-tui-payload-reader".to_string())
        .spawn(move || loop {
            let event = session.recv_event();
            let terminal_event = matches!(
                event,
                Ok(PayloadEvent::Exit(_) | PayloadEvent::Failure(_)) | Err(_)
            );
            if payload_tx.send(event).is_err() || terminal_event {
                break;
            }
        })
        .map_err(PayloadClientError::Io)?;

    loop {
        let mut redraw = false;
        while let Ok(payload_event) = payload_rx.try_recv() {
            match payload_event? {
                PayloadEvent::Output(bytes) => {
                    view.process_output(&bytes);
                    redraw = true;
                }
                PayloadEvent::Exit(exit_code) => {
                    view.set_phase(SessionPhase::Exited(exit_code));
                    terminal.draw(|frame| view.render(frame))?;
                    return Ok(exit_code);
                }
                PayloadEvent::Failure(message) => {
                    view.set_phase(SessionPhase::Error(message.clone()));
                    terminal.draw(|frame| view.render(frame))?;
                    return Err(PayloadClientError::Protocol(message));
                }
            }
        }

        if event::poll(INPUT_POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) => match view.handle_wrapper_key(key) {
                    WrapperKeyOutcome::GuestInput(input) => {
                        handle_guest_input(input, &mut writer)?;
                    }
                    WrapperKeyOutcome::PromptFinished | WrapperKeyOutcome::Redraw => {
                        redraw = true;
                    }
                    WrapperKeyOutcome::Ignored => {}
                },
                Event::Paste(text) => {
                    writer.send_input(text.as_bytes())?;
                }
                Event::Resize(cols, rows) => {
                    let layout = viewport_layout(Rect::new(0, 0, cols, rows));
                    view.set_guest_size(layout.guest_rows, layout.guest_cols);
                    writer.send_resize(layout.guest_rows, layout.guest_cols)?;
                    redraw = true;
                }
                _ => {}
            }
        }

        if redraw {
            terminal.draw(|frame| view.render(frame))?;
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;

    #[test]
    fn viewport_size_uses_terminal_area_above_status() {
        let layout = viewport_layout(Rect::new(0, 0, 80, 24));

        assert_eq!(layout.terminal, Rect::new(0, 0, 80, 23));
        assert_eq!(layout.status, Rect::new(0, 23, 80, 1));
        assert_eq!((layout.guest_rows, layout.guest_cols), (23, 80));
    }

    #[test]
    fn viewport_size_clamps_tiny_terminals() {
        let layout = viewport_layout(Rect::new(0, 0, 1, 1));

        assert_eq!((layout.guest_rows, layout.guest_cols), (1, 1));
    }

    #[test]
    fn renders_guest_output_through_tui_term_widget() {
        let backend = TestBackend::new(24, 6);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut view = GuestTerminalView::new(4, 22);
        view.process_output(b"hello\r\n\x1b[31mred");

        terminal.draw(|frame| view.render(frame)).expect("draw");

        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(contents.contains("hello"));
        assert!(contents.contains("red"));
    }

    #[test]
    fn status_bar_formats_and_truncates_to_width() {
        let mut status = StatusBar::new(21, 78);
        assert_eq!(status.text(80), "starting payload | focus guest | 78x21");
        assert_eq!(status.text(12), "starting ...");
        assert_eq!(status.text(2), "st");

        status.phase = SessionPhase::Exited(7);
        assert_eq!(status.text(80), "exited 7 | focus guest | 78x21");
    }

    #[test]
    fn key_events_translate_to_guest_bytes() {
        assert_eq!(
            key_event_to_guest_input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
            Some(GuestInput::Bytes(b"x".to_vec()))
        );
        assert_eq!(
            key_event_to_guest_input(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(GuestInput::Bytes(b"\r".to_vec()))
        );
        assert_eq!(
            key_event_to_guest_input(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Some(GuestInput::Bytes(b"\x1b[A".to_vec()))
        );
        assert_eq!(
            key_event_to_guest_input(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(GuestInput::Bytes(vec![4]))
        );
    }

    #[test]
    fn ctrl_c_becomes_guest_signal_and_wrapper_prefix_is_reserved() {
        assert_eq!(
            key_event_to_guest_input(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(GuestInput::Signal(libc::SIGINT))
        );
        assert_eq!(
            key_event_to_guest_input(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL)),
            Some(GuestInput::Reserved)
        );
    }

    #[test]
    fn wrapper_prefix_opens_prompt_without_guest_input() {
        let mut view = GuestTerminalView::new(10, 20);

        assert_eq!(
            view.handle_wrapper_key(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL)),
            WrapperKeyOutcome::Redraw
        );
        assert_eq!(
            view.handle_wrapper_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            WrapperKeyOutcome::Redraw
        );
        assert!(view.prompt.is_some());
        assert_eq!(view.status.focus, FocusMode::WrapperPrompt);
    }

    #[test]
    fn prompt_accept_and_cancel_produce_structured_results() {
        let mut view = GuestTerminalView::new(10, 20);
        view.open_prompt("decision");

        assert_eq!(
            view.handle_wrapper_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            WrapperKeyOutcome::Redraw
        );
        assert_eq!(
            view.handle_wrapper_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            WrapperKeyOutcome::PromptFinished
        );
        assert_eq!(
            view.last_prompt_result,
            Some(PromptResult::Accepted {
                value: "y".to_string()
            })
        );
        assert_eq!(view.status.focus, FocusMode::Guest);

        view.open_prompt("decision");
        assert_eq!(
            view.handle_wrapper_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            WrapperKeyOutcome::PromptFinished
        );
        assert_eq!(view.last_prompt_result, Some(PromptResult::Cancelled));
    }

    #[test]
    fn startup_dialog_defaults_to_codex_and_accepts_choices() {
        let mut dialog = StartupDialog::new();

        assert_eq!(
            dialog.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            StartupDialogResult::Accepted(StartupSelection { enable_codex: true })
        );
        assert_eq!(
            dialog.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            StartupDialogResult::Accepted(StartupSelection {
                enable_codex: false
            })
        );
    }

    #[test]
    fn startup_dialog_can_toggle_and_cancel() {
        let mut dialog = StartupDialog::new();

        assert_eq!(
            dialog.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
            StartupDialogResult::Redraw
        );
        assert!(!dialog.enable_codex);
        assert_eq!(
            dialog.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            StartupDialogResult::Cancelled
        );
    }

    #[test]
    fn resize_uses_terminal_area_above_status() {
        let layout = viewport_layout(Rect::new(0, 0, 100, 40));

        assert_eq!((layout.guest_rows, layout.guest_cols), (39, 100));
    }
}
