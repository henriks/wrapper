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
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;
use tui_term::widget::PseudoTerminal;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    ConfigCommand, ConfigNetworkMode, ConfigPort, ConfigShare, ConfigShareAccess, SetupTool,
    WrapperSandboxConfig,
};

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
            "Initialize Codex for this sandbox?\n\nCodex state will be mounted read-write at its normal home path.\n\n[Y] yes  [N] no  [Space] toggle  [Enter] accept  [Esc] cancel\n\nCurrent: {choice}"
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigEditorResult {
    Save,
    Cancel,
    Redraw,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigEditor {
    config: WrapperSandboxConfig,
}

impl ConfigEditor {
    fn new(config: WrapperSandboxConfig) -> Self {
        Self { config }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ConfigEditorResult {
        if key.kind == KeyEventKind::Release {
            return ConfigEditorResult::Ignored;
        }
        match key.code {
            KeyCode::Char('s') | KeyCode::Enter => ConfigEditorResult::Save,
            KeyCode::Esc | KeyCode::Char('q') => ConfigEditorResult::Cancel,
            KeyCode::Char('c') => {
                self.cycle_default_command();
                ConfigEditorResult::Redraw
            }
            KeyCode::Char('n') => {
                self.cycle_network_mode();
                ConfigEditorResult::Redraw
            }
            KeyCode::Char('g') => {
                self.config.auth.github = !self.config.auth.github;
                ConfigEditorResult::Redraw
            }
            KeyCode::Char('d') => {
                self.toggle_default_rw_share();
                ConfigEditorResult::Redraw
            }
            KeyCode::Char('p') => {
                self.toggle_sample_port();
                ConfigEditorResult::Redraw
            }
            _ => ConfigEditorResult::Ignored,
        }
    }

    fn cycle_default_command(&mut self) {
        match self.config.default_command.command.as_str() {
            "codex" => self.apply_setup_tool(SetupTool::Pi),
            "pi" => {
                self.config.default_command = ConfigCommand::new("bash");
                self.config.shares.clear();
            }
            _ => self.apply_setup_tool(SetupTool::Codex),
        }
    }

    fn apply_setup_tool(&mut self, tool: SetupTool) {
        if let Ok(mut config) = WrapperSandboxConfig::setup_tool(tool) {
            config.network = self.config.network.clone();
            config.auth = self.config.auth.clone();
            config.published_ports = self.config.published_ports.clone();
            self.config = config;
        }
    }

    fn cycle_network_mode(&mut self) {
        self.config.network.mode = match self.config.network.mode {
            ConfigNetworkMode::Public => ConfigNetworkMode::None,
            ConfigNetworkMode::None => {
                if self.config.network.allowed_domains.is_empty() {
                    self.config
                        .network
                        .allowed_domains
                        .push("example.com".to_string());
                }
                ConfigNetworkMode::Allowlist
            }
            ConfigNetworkMode::Allowlist => ConfigNetworkMode::Public,
        };
    }

    fn toggle_default_rw_share(&mut self) {
        if let Some(index) = self
            .config
            .shares
            .iter()
            .position(|share| share.host_path == "/tmp/agentvm-share")
        {
            self.config.shares.remove(index);
        } else {
            self.config.shares.push(ConfigShare {
                host_path: "/tmp/agentvm-share".to_string(),
                guest_path: None,
                access: ConfigShareAccess::Rw,
                required: false,
                shadows: Vec::new(),
            });
        }
    }

    fn toggle_sample_port(&mut self) {
        if let Some(index) = self
            .config
            .published_ports
            .iter()
            .position(|port| port.host == 18080 && port.guest == 8080)
        {
            self.config.published_ports.remove(index);
        } else {
            self.config.published_ports.push(ConfigPort {
                host: 18080,
                guest: 8080,
            });
        }
    }

    fn render(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area(), 78, 18);
        let network = match self.config.network.mode {
            ConfigNetworkMode::Public => "public",
            ConfigNetworkMode::None => "none",
            ConfigNetworkMode::Allowlist => "allowlist",
        };
        let body = format!(
            "Default: {} {}\nNetwork: {network}\nAllowlist: {}\nGitHub auth: {}\nAWS profile: {}\nShares: {}\nPublished ports: {}\n\n[C] command  [N] network  [G] github  [D] sample rw share  [P] sample port\n[Enter/S] save  [Esc/Q] cancel",
            self.config.default_command.command,
            self.config.default_command.args.join(" "),
            self.config.network.allowed_domains.join(", "),
            if self.config.auth.github { "on" } else { "off" },
            self.config.auth.aws_profile.as_deref().unwrap_or("none"),
            self.config.shares.len(),
            self.config.published_ports.len(),
        );
        let paragraph = Paragraph::new(body)
            .block(
                Block::default()
                    .title(" Sandbox Config ")
                    .borders(Borders::ALL),
            )
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
    }
}

pub(crate) fn run_config_editor(
    config: WrapperSandboxConfig,
) -> Result<WrapperSandboxConfig, String> {
    let mut terminal = ratatui::try_init().map_err(|error| error.to_string())?;
    let _restore = RestoreTerminal;
    let mut editor = ConfigEditor::new(config);
    terminal
        .draw(|frame| editor.render(frame))
        .map_err(|error| error.to_string())?;
    loop {
        if event::poll(INPUT_POLL_INTERVAL).map_err(|error| error.to_string())? {
            let event = event::read().map_err(|error| error.to_string())?;
            if let Event::Key(key) = event {
                match editor.handle_key(key) {
                    ConfigEditorResult::Save => return Ok(editor.config),
                    ConfigEditorResult::Cancel => return Err("config edit cancelled".to_string()),
                    ConfigEditorResult::Redraw => terminal
                        .draw(|frame| editor.render(frame))
                        .map_err(|error| error.to_string())
                        .map(|_| ())?,
                    ConfigEditorResult::Ignored => {}
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
                    value: prompt.input.value().to_string(),
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
            _ => {
                let event = Event::Key(key);
                if prompt.input.handle_event(&event).is_some() {
                    WrapperKeyOutcome::Redraw
                } else {
                    WrapperKeyOutcome::Ignored
                }
            }
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

#[derive(Debug, Clone)]
struct PromptState {
    question: String,
    input: Input,
}

impl PromptState {
    fn new(question: impl Into<String>) -> Self {
        Self {
            question: question.into(),
            input: Input::default(),
        }
    }

    fn text(&self, width: u16) -> String {
        truncate_status(
            format!("prompt | {}: {}", self.question, self.input.value()),
            width,
        )
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

fn truncate_status(text: String, width: u16) -> String {
    let width = usize::from(width);
    if UnicodeWidthStr::width(text.as_str()) <= width {
        return text;
    }
    if width == 0 {
        return String::new();
    }
    if width <= 3 {
        return take_display_width(&text, width);
    }
    let keep = width - 3;
    let mut truncated = take_display_width(&text, keep);
    truncated.push_str("...");
    truncated
}

fn take_display_width(text: &str, width: usize) -> String {
    let mut used = 0;
    let mut output = String::new();
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        output.push(ch);
        used += ch_width;
    }
    output
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

    fn backend_rows(terminal: &Terminal<TestBackend>, width: u16, height: u16) -> Vec<String> {
        let cells = &terminal.backend().buffer().content;
        cells
            .chunks(usize::from(width))
            .take(usize::from(height))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect()
    }

    fn backend_text(terminal: &Terminal<TestBackend>, width: u16, height: u16) -> String {
        backend_rows(terminal, width, height).join("\n")
    }

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
    fn status_truncation_uses_display_width() {
        assert_eq!(truncate_status("øøøø".to_string(), 4), "øøøø");
        assert_eq!(truncate_status("界界xy".to_string(), 6), "界界xy");
        assert_eq!(truncate_status("界界xy".to_string(), 5), "界...");
        assert_eq!(truncate_status("界界xy".to_string(), 4), "...");
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
    fn startup_dialog_renders_at_representative_terminal_sizes() {
        for (width, height) in [(80, 24), (42, 12)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");
            let dialog = StartupDialog::new();

            terminal.draw(|frame| dialog.render(frame)).expect("draw");

            let text = backend_text(&terminal, width, height);
            assert!(text.contains("Sandbox Setup"), "{width}x{height}\n{text}");
            assert!(
                text.contains("Initialize Codex"),
                "{width}x{height}\n{text}"
            );
            assert!(text.contains("Current: yes"), "{width}x{height}\n{text}");
            assert_eq!(
                backend_rows(&terminal, width, height).len(),
                usize::from(height)
            );
        }
    }

    #[test]
    fn config_editor_model_edits_main_config_fields_before_save() {
        let mut editor = ConfigEditor::new(
            WrapperSandboxConfig::setup_tool(SetupTool::Codex).expect("setup config"),
        );

        assert_eq!(
            editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)),
            ConfigEditorResult::Redraw
        );
        assert_eq!(editor.config.default_command.command, "pi");
        let pi_share = editor.config.shares.first().expect("pi share");
        assert!(pi_share.host_path.ends_with("/.pi"));
        assert_eq!(pi_share.access, ConfigShareAccess::Rw);

        assert_eq!(
            editor.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            ConfigEditorResult::Redraw
        );
        assert_eq!(editor.config.network.mode, ConfigNetworkMode::None);
        assert_eq!(
            editor.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            ConfigEditorResult::Redraw
        );
        assert_eq!(editor.config.network.mode, ConfigNetworkMode::Allowlist);
        assert_eq!(
            editor.config.network.allowed_domains,
            vec!["example.com".to_string()]
        );

        assert_eq!(
            editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)),
            ConfigEditorResult::Redraw
        );
        assert!(editor.config.auth.github);

        assert_eq!(
            editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
            ConfigEditorResult::Redraw
        );
        assert_eq!(editor.config.shares.len(), 2);

        assert_eq!(
            editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            ConfigEditorResult::Redraw
        );
        assert_eq!(editor.config.published_ports.len(), 1);

        assert_eq!(
            editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ConfigEditorResult::Save
        );
    }

    #[test]
    fn config_editor_renders_fixed_size_summary_without_overlap() {
        let mut editor = ConfigEditor::new(
            WrapperSandboxConfig::setup_tool(SetupTool::Codex).expect("setup config"),
        );
        editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));

        for (width, height) in [(82, 24), (50, 14)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");

            terminal.draw(|frame| editor.render(frame)).expect("draw");

            let text = backend_text(&terminal, width, height);
            assert!(text.contains("Sandbox Config"), "{width}x{height}\n{text}");
            assert!(text.contains("Default: pi"), "{width}x{height}\n{text}");
            assert!(text.contains("Network: none"), "{width}x{height}\n{text}");
            assert!(text.contains("GitHub auth: on"), "{width}x{height}\n{text}");
            assert!(text.contains("Shares: 2"), "{width}x{height}\n{text}");
            assert!(
                text.contains("Published ports: 1"),
                "{width}x{height}\n{text}"
            );
            assert_eq!(
                backend_rows(&terminal, width, height).len(),
                usize::from(height)
            );
        }
    }

    #[test]
    fn prompt_focus_keeps_text_out_of_guest_input_until_closed() {
        let mut view = GuestTerminalView::new(10, 20);
        view.open_prompt("wrapper prompt");

        assert_eq!(view.status.focus, FocusMode::WrapperPrompt);
        assert_eq!(
            view.handle_wrapper_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
            WrapperKeyOutcome::Redraw
        );
        assert_eq!(view.prompt.as_ref().expect("prompt").input.value(), "x");
        assert_eq!(view.last_prompt_result, None);
        assert_ne!(
            view.handle_wrapper_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            WrapperKeyOutcome::GuestInput(GuestInput::Signal(libc::SIGINT))
        );

        assert_eq!(
            view.handle_wrapper_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            WrapperKeyOutcome::PromptFinished
        );
        assert_eq!(view.status.focus, FocusMode::Guest);
        assert_eq!(
            view.last_prompt_result,
            Some(PromptResult::Accepted {
                value: "x".to_string()
            })
        );
    }

    #[test]
    fn resize_uses_terminal_area_above_status() {
        let layout = viewport_layout(Rect::new(0, 0, 100, 40));

        assert_eq!((layout.guest_rows, layout.guest_cols), (39, 100));
    }
}
