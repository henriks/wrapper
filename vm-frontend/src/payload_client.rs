use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{
    atomic::{AtomicBool, AtomicI32, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

const FRAME_HEADER_LEN: usize = 5;
const MAX_FRAME_PAYLOAD: usize = 16 * 1024 * 1024;

#[cfg(unix)]
static SIGNAL_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PayloadRequest {
    pub script: String,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub rows: u16,
    pub cols: u16,
}

impl PayloadRequest {
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            cwd: "/".to_string(),
            env: BTreeMap::new(),
            rows: 24,
            cols: 80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticRequest {
    pub script: String,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub timeout_seconds: u64,
    pub max_output_bytes: u64,
}

impl DiagnosticRequest {
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            cwd: "/".to_string(),
            env: BTreeMap::new(),
            timeout_seconds: 10,
            max_output_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum PayloadClientError {
    Io(io::Error),
    Json(serde_json::Error),
    Protocol(String),
    Address(String),
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadControlOptions {
    pub forward_signals: bool,
    pub forward_resize: bool,
}

impl PayloadControlOptions {
    pub fn interactive() -> Self {
        Self {
            forward_signals: true,
            forward_resize: true,
        }
    }

    pub fn disabled() -> Self {
        Self {
            forward_signals: false,
            forward_resize: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadEvent {
    Output(Vec<u8>),
    Exit(i32),
    Failure(String),
}

pub struct PayloadSession<S> {
    stream: S,
}

impl<S: Read + Write> PayloadSession<S> {
    pub fn from_stream(
        mut stream: S,
        request: &PayloadRequest,
    ) -> Result<Self, PayloadClientError> {
        let request_json = serde_json::to_vec(request)?;
        send_frame(&mut stream, b'R', &request_json)?;
        Ok(Self { stream })
    }

    pub fn send_input(&mut self, input: &[u8]) -> Result<(), PayloadClientError> {
        send_frame(&mut self.stream, b'I', input)?;
        Ok(())
    }

    pub fn send_signal(&mut self, signal: i32) -> Result<(), PayloadClientError> {
        send_signal_frame(&mut self.stream, signal)?;
        Ok(())
    }

    pub fn send_resize(&mut self, rows: u16, cols: u16) -> Result<(), PayloadClientError> {
        send_resize_frame(&mut self.stream, rows, cols)?;
        Ok(())
    }

    pub fn recv_event(&mut self) -> Result<PayloadEvent, PayloadClientError> {
        payload_event_from_stream(&mut self.stream)
    }
}

impl PayloadSession<TcpStream> {
    pub fn connect(
        addr: impl ToSocketAddrs,
        request: &PayloadRequest,
    ) -> Result<Self, PayloadClientError> {
        let stream = connect_payload(addr, Duration::from_secs(10))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
        Self::from_stream(stream, request)
    }

    pub fn try_clone_writer(&self) -> Result<PayloadWriter, PayloadClientError> {
        Ok(PayloadWriter {
            stream: self.stream.try_clone()?,
        })
    }
}

pub struct PayloadWriter {
    stream: TcpStream,
}

impl PayloadWriter {
    pub fn send_input(&mut self, input: &[u8]) -> Result<(), PayloadClientError> {
        send_frame(&mut self.stream, b'I', input)?;
        Ok(())
    }

    pub fn send_signal(&mut self, signal: i32) -> Result<(), PayloadClientError> {
        send_signal_frame(&mut self.stream, signal)?;
        Ok(())
    }

    pub fn send_resize(&mut self, rows: u16, cols: u16) -> Result<(), PayloadClientError> {
        send_resize_frame(&mut self.stream, rows, cols)?;
        Ok(())
    }
}

pub fn ping_payload(addr: impl ToSocketAddrs) -> Result<(), PayloadClientError> {
    let mut stream = connect_payload(addr, Duration::from_secs(1))?;
    send_frame(&mut stream, b'P', &[])?;
    let (frame_type, payload) = recv_frame(&mut stream)?;
    if frame_type == b'K' && payload == b"ok" {
        return Ok(());
    }
    Err(PayloadClientError::Protocol(format!(
        "unexpected ping response: frame={frame_type:?} payload={}",
        String::from_utf8_lossy(&payload)
    )))
}

pub fn run_payload_tcp(
    addr: impl ToSocketAddrs,
    request: &PayloadRequest,
    input: Option<Box<dyn Read + Send>>,
    output: &mut impl Write,
) -> Result<i32, PayloadClientError> {
    run_payload_tcp_with_control(
        addr,
        request,
        input,
        output,
        PayloadControlOptions::disabled(),
    )
}

pub fn run_diagnostic_tcp(
    addr: impl ToSocketAddrs,
    request: &DiagnosticRequest,
    output: &mut impl Write,
) -> Result<i32, PayloadClientError> {
    let mut stream = connect_payload(addr, Duration::from_secs(10))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    run_diagnostic_io(&mut stream, request, output)
}

pub fn run_payload_tcp_with_control(
    addr: impl ToSocketAddrs,
    request: &PayloadRequest,
    input: Option<Box<dyn Read + Send>>,
    output: &mut impl Write,
    control: PayloadControlOptions,
) -> Result<i32, PayloadClientError> {
    let mut session = PayloadSession::connect(addr, request)?;
    run_payload_session(&mut session, input, output, control)
}

fn connect_payload(
    addr: impl ToSocketAddrs,
    timeout: Duration,
) -> Result<TcpStream, PayloadClientError> {
    let mut last_error = None;
    for addr in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(PayloadClientError::Io(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no socket address resolved")
    })))
}

pub fn socket_addr(host: &str, port: u16) -> Result<SocketAddr, PayloadClientError> {
    (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| PayloadClientError::Address(format!("no socket address for {host}:{port}")))
}

fn run_payload_session(
    session: &mut PayloadSession<TcpStream>,
    input: Option<Box<dyn Read + Send>>,
    output: &mut impl Write,
    control: PayloadControlOptions,
) -> Result<i32, PayloadClientError> {
    let done = Arc::new(AtomicBool::new(false));
    let send_lock = Arc::new(Mutex::new(session.stream.try_clone()?));

    if let Some(mut input) = input {
        let done = done.clone();
        let send_lock = send_lock.clone();
        thread::Builder::new()
            .name("agentvm-payload-stdin".to_string())
            .spawn(move || {
                let mut buffer = [0; 64 * 1024];
                while !done.load(Ordering::SeqCst) {
                    let Ok(count) = input.read(&mut buffer) else {
                        return;
                    };
                    if count == 0 {
                        return;
                    }
                    let Ok(mut writer) = send_lock.lock() else {
                        return;
                    };
                    if send_frame(&mut *writer, b'I', &buffer[..count]).is_err() {
                        return;
                    }
                }
            })
            .map_err(PayloadClientError::Io)?;
    }
    let signal_forwarder = SignalForwarder::install(control, send_lock.clone(), done.clone())?;

    let result = (|| -> Result<i32, PayloadClientError> {
        loop {
            match session.recv_event()? {
                PayloadEvent::Output(payload) => {
                    output.write_all(&payload)?;
                    output.flush()?;
                }
                PayloadEvent::Exit(exit_code) => return Ok(exit_code),
                PayloadEvent::Failure(message) => {
                    return Err(PayloadClientError::Protocol(message))
                }
            }
        }
    })();
    done.store(true, Ordering::SeqCst);
    drop(signal_forwarder);
    result
}

#[cfg(test)]
fn run_payload_io(
    stream: &mut (impl Read + Write),
    request: &PayloadRequest,
    output: &mut impl Write,
) -> Result<i32, PayloadClientError> {
    let mut session = PayloadSession::from_stream(stream, request)?;
    loop {
        match session.recv_event()? {
            PayloadEvent::Output(payload) => {
                output.write_all(&payload)?;
                output.flush()?;
            }
            PayloadEvent::Exit(exit_code) => return Ok(exit_code),
            PayloadEvent::Failure(message) => return Err(PayloadClientError::Protocol(message)),
        }
    }
}

fn run_diagnostic_io(
    stream: &mut (impl Read + Write),
    request: &DiagnosticRequest,
    output: &mut impl Write,
) -> Result<i32, PayloadClientError> {
    let request_json = serde_json::to_vec(request)?;
    send_frame(stream, b'D', &request_json)?;
    loop {
        match payload_event_from_stream(stream)? {
            PayloadEvent::Output(payload) => {
                output.write_all(&payload)?;
                output.flush()?;
            }
            PayloadEvent::Exit(exit_code) => return Ok(exit_code),
            PayloadEvent::Failure(message) => return Err(PayloadClientError::Protocol(message)),
        }
    }
}

fn payload_event_from_stream(stream: &mut impl Read) -> Result<PayloadEvent, PayloadClientError> {
    let (frame_type, payload) = recv_frame(stream)?;
    match frame_type {
        b'O' => Ok(PayloadEvent::Output(payload)),
        b'X' => exit_code_from_payload(&payload).map(PayloadEvent::Exit),
        b'F' => Ok(PayloadEvent::Failure(
            String::from_utf8_lossy(&payload).to_string(),
        )),
        other => Err(PayloadClientError::Protocol(format!(
            "unexpected payload frame type {other:?}"
        ))),
    }
}

fn send_signal_frame(writer: &mut impl Write, signal: i32) -> io::Result<()> {
    let payload = serde_json::json!({ "signal": signal }).to_string();
    send_frame(writer, b'S', payload.as_bytes())
}

fn send_resize_frame(writer: &mut impl Write, rows: u16, cols: u16) -> io::Result<()> {
    let payload = serde_json::json!({ "rows": rows, "cols": cols }).to_string();
    send_frame(writer, b'W', payload.as_bytes())
}

struct SignalForwarder {
    #[cfg(unix)]
    _inner: Option<UnixSignalForwarder>,
}

impl SignalForwarder {
    fn install(
        control: PayloadControlOptions,
        send_lock: Arc<Mutex<TcpStream>>,
        done: Arc<AtomicBool>,
    ) -> Result<Self, PayloadClientError> {
        if !control.forward_signals && !control.forward_resize {
            return Ok(Self {
                #[cfg(unix)]
                _inner: None,
            });
        }

        #[cfg(unix)]
        {
            return Ok(Self {
                _inner: Some(UnixSignalForwarder::install(control, send_lock, done)?),
            });
        }

        #[cfg(not(unix))]
        {
            let _ = (send_lock, done);
            Err(PayloadClientError::Unsupported(
                "payload signal forwarding is only supported on Unix hosts".to_string(),
            ))
        }
    }
}

#[cfg(unix)]
struct UnixSignalForwarder {
    read_fd: i32,
    write_fd: i32,
    old_actions: Vec<(i32, libc::sigaction)>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(unix)]
impl UnixSignalForwarder {
    fn install(
        control: PayloadControlOptions,
        send_lock: Arc<Mutex<TcpStream>>,
        done: Arc<AtomicBool>,
    ) -> Result<Self, PayloadClientError> {
        let mut fds = [0; 2];
        // SAFETY: pipe initializes both fd slots on success.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(PayloadClientError::Io(io::Error::last_os_error()));
        }
        let read_fd = fds[0];
        let write_fd = fds[1];
        SIGNAL_WRITE_FD.store(write_fd, Ordering::SeqCst);

        let mut old_actions = Vec::new();
        for signal in control.signals() {
            let old = install_signal_handler(signal)?;
            old_actions.push((signal, old));
        }

        let thread = thread::Builder::new()
            .name("agentvm-payload-signals".to_string())
            .spawn(move || signal_forward_loop(read_fd, send_lock, done))
            .map_err(PayloadClientError::Io)?;

        Ok(Self {
            read_fd,
            write_fd,
            old_actions,
            thread: Some(thread),
        })
    }
}

#[cfg(unix)]
impl Drop for UnixSignalForwarder {
    fn drop(&mut self) {
        for (signal, old) in &self.old_actions {
            // SAFETY: old action was returned by sigaction for this signal.
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
        SIGNAL_WRITE_FD.store(-1, Ordering::SeqCst);
        // SAFETY: fds are owned by this forwarder and closed exactly once here.
        unsafe {
            let _ = libc::close(self.write_fd);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // SAFETY: fd is owned by this forwarder and closed exactly once here.
        unsafe {
            let _ = libc::close(self.read_fd);
        }
    }
}

#[cfg(unix)]
impl PayloadControlOptions {
    fn signals(self) -> Vec<i32> {
        let mut signals = Vec::new();
        if self.forward_signals {
            signals.extend([libc::SIGINT, libc::SIGTERM, libc::SIGHUP]);
        }
        if self.forward_resize {
            signals.push(libc::SIGWINCH);
        }
        signals
    }
}

#[cfg(unix)]
fn install_signal_handler(signal: i32) -> Result<libc::sigaction, PayloadClientError> {
    // SAFETY: zeroed sigaction is immediately initialized before use.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = payload_signal_handler as *const () as usize;
    action.sa_flags = 0;
    // SAFETY: sigemptyset initializes the mask field.
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }

    // SAFETY: zeroed sigaction receives the previous handler from sigaction.
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: pointers reference valid sigaction structures.
    if unsafe { libc::sigaction(signal, &action, &mut old) } != 0 {
        return Err(PayloadClientError::Io(io::Error::last_os_error()));
    }
    Ok(old)
}

#[cfg(unix)]
extern "C" fn payload_signal_handler(signal: i32) {
    let fd = SIGNAL_WRITE_FD.load(Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    let bytes = signal.to_ne_bytes();
    // SAFETY: write is async-signal-safe; fd is managed by UnixSignalForwarder.
    unsafe {
        let _ = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
    }
}

#[cfg(unix)]
fn signal_forward_loop(read_fd: i32, send_lock: Arc<Mutex<TcpStream>>, done: Arc<AtomicBool>) {
    let mut buffer = [0_u8; std::mem::size_of::<i32>()];
    while !done.load(Ordering::SeqCst) {
        // SAFETY: buffer points to valid writable memory for the requested size.
        let read = unsafe { libc::read(read_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read <= 0 {
            return;
        }
        if read as usize != buffer.len() {
            continue;
        }
        let signal = i32::from_ne_bytes(buffer);
        let Ok(mut writer) = send_lock.lock() else {
            return;
        };
        let result = if signal == libc::SIGWINCH {
            let (rows, cols) = terminal_size();
            send_resize_frame(&mut *writer, rows, cols)
        } else {
            send_signal_frame(&mut *writer, signal)
        };
        if result.is_err() {
            return;
        }
    }
}

#[cfg(unix)]
pub fn terminal_size() -> (u16, u16) {
    // SAFETY: zeroed winsize is filled by ioctl on success.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: ioctl writes to a valid winsize pointer.
    let ok = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
    if ok && size.ws_row > 0 && size.ws_col > 0 {
        (size.ws_row, size.ws_col)
    } else {
        (24, 80)
    }
}

#[cfg(not(unix))]
pub fn terminal_size() -> (u16, u16) {
    (24, 80)
}

fn exit_code_from_payload(payload: &[u8]) -> Result<i32, PayloadClientError> {
    #[derive(Deserialize)]
    struct ExitFrame {
        exit_code: i32,
    }

    Ok(serde_json::from_slice::<ExitFrame>(payload)?.exit_code)
}

fn send_frame(writer: &mut impl Write, frame_type: u8, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "payload frame too large: {} > {}",
                payload.len(),
                MAX_FRAME_PAYLOAD
            ),
        ));
    }
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "payload frame too large"))?;
    writer.write_all(&[frame_type])?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(payload)
}

fn recv_frame(reader: &mut impl Read) -> Result<(u8, Vec<u8>), PayloadClientError> {
    let mut header = [0; FRAME_HEADER_LEN];
    reader.read_exact(&mut header)?;
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_FRAME_PAYLOAD {
        return Err(PayloadClientError::Protocol(format!(
            "payload frame too large: {len} > {MAX_FRAME_PAYLOAD}"
        )));
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload)?;
    Ok((header[0], payload))
}

impl From<io::Error> for PayloadClientError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PayloadClientError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl std::fmt::Display for PayloadClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayloadClientError::Io(error) => write!(f, "payload client IO failed: {error}"),
            PayloadClientError::Json(error) => write!(f, "payload client JSON failed: {error}"),
            PayloadClientError::Protocol(error) => write!(f, "payload protocol failed: {error}"),
            PayloadClientError::Address(error) => write!(f, "{error}"),
            PayloadClientError::Unsupported(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PayloadClientError {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 64,
            max_shrink_iters: 32,
            ..ProptestConfig::default()
        })]

        #[test]
        fn proptest_payload_frames_round_trip(frame_type in any::<u8>(), payload in prop::collection::vec(any::<u8>(), 0..=1024)) {
            let mut bytes = Vec::new();
            send_frame(&mut bytes, frame_type, &payload).expect("send");
            let (decoded_type, decoded_payload) = recv_frame(&mut io::Cursor::new(bytes)).expect("recv");

            prop_assert_eq!(decoded_type, frame_type);
            prop_assert_eq!(decoded_payload, payload);
        }

        #[test]
        fn proptest_arbitrary_payload_frame_bytes_stay_bounded(bytes in prop::collection::vec(any::<u8>(), 0..=128)) {
            let _ = recv_frame(&mut io::Cursor::new(bytes));
        }
    }

    #[test]
    fn frame_round_trip_uses_big_endian_length() {
        let mut bytes = Vec::new();

        send_frame(&mut bytes, b'P', b"hello").expect("send");

        assert_eq!(&bytes[..5], &[b'P', 0, 0, 0, 5]);
        let (frame_type, payload) = recv_frame(&mut io::Cursor::new(bytes)).expect("recv");
        assert_eq!(frame_type, b'P');
        assert_eq!(payload, b"hello");
    }

    #[test]
    fn run_payload_sends_request_streams_output_and_returns_exit_code() {
        let mut server_frames = Vec::new();
        send_frame(&mut server_frames, b'O', b"hello\r\n").expect("output");
        send_frame(&mut server_frames, b'X', br#"{"exit_code":7}"#).expect("exit");
        let mut stream = ScriptedIo::new(server_frames);

        let request = PayloadRequest::new("echo hello");
        let mut output = Vec::new();
        let exit_code = run_payload_io(&mut stream, &request, &mut output).expect("payload run");

        assert_eq!(exit_code, 7);
        assert_eq!(output, b"hello\r\n");
        let (frame_type, payload) =
            recv_frame(&mut io::Cursor::new(stream.written)).expect("request frame");
        assert_eq!(frame_type, b'R');
        let request: serde_json::Value = serde_json::from_slice(&payload).expect("json");
        assert_eq!(request["script"], "echo hello");
    }

    #[test]
    fn run_diagnostic_sends_diagnostic_request_and_returns_exit_code() {
        let mut server_frames = Vec::new();
        send_frame(&mut server_frames, b'O', b"diag\n").expect("output");
        send_frame(
            &mut server_frames,
            b'X',
            br#"{"exit_code":3,"diagnostic":true}"#,
        )
        .expect("exit");
        let mut stream = ScriptedIo::new(server_frames);

        let mut request = DiagnosticRequest::new("echo diag");
        request.timeout_seconds = 2;
        request.max_output_bytes = 4096;
        let mut output = Vec::new();
        let exit_code =
            run_diagnostic_io(&mut stream, &request, &mut output).expect("diagnostic run");

        assert_eq!(exit_code, 3);
        assert_eq!(output, b"diag\n");
        let (frame_type, payload) =
            recv_frame(&mut io::Cursor::new(stream.written)).expect("request frame");
        assert_eq!(frame_type, b'D');
        let request: serde_json::Value = serde_json::from_slice(&payload).expect("json");
        assert_eq!(request["script"], "echo diag");
        assert_eq!(request["timeout_seconds"], 2);
        assert_eq!(request["max_output_bytes"], 4096);
    }

    #[test]
    fn rust_client_runs_real_python_payload_server() {
        let Some((mut child, port)) = start_python_payload_server() else {
            return;
        };

        let request = PayloadRequest::new("printf rust-python-ok; exit 6");
        let mut output = Vec::new();
        let exit_code = run_payload_tcp(("127.0.0.1", port), &request, None, &mut output)
            .expect("python payload server run");

        assert_eq!(exit_code, 6);
        assert!(output
            .windows(b"rust-python-ok".len())
            .any(|window| window == b"rust-python-ok"));
        child.kill_and_wait();
    }

    #[test]
    fn python_payload_server_gives_interactive_bash_a_controlling_tty() {
        let Some((mut child, port)) = start_python_payload_server() else {
            return;
        };

        let request = PayloadRequest::new("bash --noprofile --norc -i -c 'printf bash-ready'");
        let mut output = Vec::new();
        let exit_code = run_payload_tcp(("127.0.0.1", port), &request, None, &mut output)
            .expect("interactive bash payload run");
        let output = String::from_utf8_lossy(&output);

        assert_eq!(exit_code, 0);
        assert!(output.contains("bash-ready"), "bash output was {output:?}");
        assert!(!output.contains("cannot set terminal process group"));
        assert!(!output.contains("no job control"));
        child.kill_and_wait();
    }

    #[test]
    fn diagnostic_can_run_while_primary_payload_is_active() {
        let Some((mut child, port)) = start_python_payload_server() else {
            return;
        };

        let primary = thread::spawn(move || {
            let request = PayloadRequest::new("printf primary-start; sleep 1; printf primary-done");
            let mut output = Vec::new();
            let exit_code = run_payload_tcp(("127.0.0.1", port), &request, None, &mut output)
                .expect("primary payload run");
            (exit_code, output)
        });
        thread::sleep(Duration::from_millis(200));

        let request = DiagnosticRequest::new("printf diagnostic-ok; exit 7");
        let mut output = Vec::new();
        let exit_code =
            run_diagnostic_tcp(("127.0.0.1", port), &request, &mut output).expect("diagnostic run");

        assert_eq!(exit_code, 7);
        assert_eq!(output, b"diagnostic-ok");
        let (primary_exit, primary_output) = primary.join().expect("primary thread");
        assert_eq!(primary_exit, 0);
        assert!(primary_output
            .windows(b"primary-done".len())
            .any(|window| window == b"primary-done"));
        child.kill_and_wait();
    }

    #[test]
    fn payload_session_sends_control_frames_and_receives_events() {
        let mut server_frames = Vec::new();
        send_frame(&mut server_frames, b'O', b"ready").expect("output");
        send_frame(&mut server_frames, b'X', br#"{"exit_code":0}"#).expect("exit");
        let mut stream = ScriptedIo::new(server_frames);

        let request = PayloadRequest::new("agent");
        {
            let mut session = PayloadSession::from_stream(&mut stream, &request).expect("session");
            session.send_input(b"hello").expect("input");
            session.send_resize(33, 101).expect("resize");
            session.send_signal(2).expect("signal");

            assert_eq!(
                session.recv_event().expect("output"),
                PayloadEvent::Output(b"ready".to_vec())
            );
            assert_eq!(session.recv_event().expect("exit"), PayloadEvent::Exit(0));
        }

        let mut written = io::Cursor::new(stream.written);
        let (frame_type, payload) = recv_frame(&mut written).expect("request frame");
        assert_eq!(frame_type, b'R');
        let request: serde_json::Value = serde_json::from_slice(&payload).expect("json");
        assert_eq!(request["script"], "agent");
        let (frame_type, payload) = recv_frame(&mut written).expect("input frame");
        assert_eq!(frame_type, b'I');
        assert_eq!(payload, b"hello");
        let (frame_type, payload) = recv_frame(&mut written).expect("resize frame");
        assert_eq!(frame_type, b'W');
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload).expect("resize json"),
            serde_json::json!({ "rows": 33, "cols": 101 })
        );
        let (frame_type, payload) = recv_frame(&mut written).expect("signal frame");
        assert_eq!(frame_type, b'S');
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload).expect("signal json"),
            serde_json::json!({ "signal": 2 })
        );
    }

    #[test]
    fn payload_session_surfaces_failure_events() {
        let mut server_frames = Vec::new();
        send_frame(&mut server_frames, b'F', b"payload failed").expect("failure");
        let mut stream = ScriptedIo::new(server_frames);

        let mut session = PayloadSession::from_stream(&mut stream, &PayloadRequest::new("agent"))
            .expect("session");

        assert_eq!(
            session.recv_event().expect("failure event"),
            PayloadEvent::Failure("payload failed".to_string())
        );
    }

    #[test]
    fn signal_and_resize_frames_match_guest_protocol() {
        let mut bytes = Vec::new();

        send_signal_frame(&mut bytes, 2).expect("signal");
        send_resize_frame(&mut bytes, 40, 120).expect("resize");

        let mut cursor = io::Cursor::new(bytes);
        let (frame_type, payload) = recv_frame(&mut cursor).expect("signal frame");
        assert_eq!(frame_type, b'S');
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload).expect("signal json"),
            serde_json::json!({ "signal": 2 })
        );
        let (frame_type, payload) = recv_frame(&mut cursor).expect("resize frame");
        assert_eq!(frame_type, b'W');
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload).expect("resize json"),
            serde_json::json!({ "rows": 40, "cols": 120 })
        );
    }

    #[test]
    fn socket_addr_resolves_host_and_port() {
        let addr = socket_addr("127.0.0.1", 0).expect("loopback");
        assert_eq!(addr.port(), 0);
    }

    #[test]
    fn frame_length_limit_rejects_oversized_send_and_receive() {
        let payload = vec![0; MAX_FRAME_PAYLOAD + 1];
        let error = send_frame(&mut Vec::new(), b'I', &payload).expect_err("oversized send");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("payload frame too large"));

        let mut bytes = vec![b'O'];
        bytes.extend_from_slice(&((MAX_FRAME_PAYLOAD as u32) + 1).to_be_bytes());
        let error = recv_frame(&mut io::Cursor::new(bytes)).expect_err("oversized receive");
        assert!(
            matches!(error, PayloadClientError::Protocol(message) if message.contains("payload frame too large"))
        );
    }

    #[test]
    fn truncated_frame_reports_io_without_allocating_payload() {
        let mut bytes = vec![b'O'];
        bytes.extend_from_slice(&4u32.to_be_bytes());
        bytes.extend_from_slice(b"ab");

        let error = recv_frame(&mut io::Cursor::new(bytes)).expect_err("truncated");
        assert!(
            matches!(error, PayloadClientError::Io(io_error) if io_error.kind() == io::ErrorKind::UnexpectedEof)
        );
    }

    #[test]
    fn exit_code_payload_is_required_json() {
        let error = exit_code_from_payload(b"not-json").expect_err("invalid");
        assert!(matches!(error, PayloadClientError::Json(_)));
    }

    fn start_python_payload_server() -> Option<(ChildGuard, u16)> {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        let server_path = repo_root.join("docker/guest-payload-server.py");
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);

        let child = match std::process::Command::new("python3")
            .arg(server_path)
            .arg("--tcp-host")
            .arg("127.0.0.1")
            .arg("--tcp-port")
            .arg(port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => panic!("spawn python payload server: {error}"),
        };
        let child = ChildGuard(Some(child));

        let mut ready = false;
        for _ in 0..50 {
            if ping_payload(("127.0.0.1", port)).is_ok() {
                ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(ready, "python payload server did not become ready");

        Some((child, port))
    }

    struct ChildGuard(Option<std::process::Child>);

    impl ChildGuard {
        fn kill_and_wait(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            self.kill_and_wait();
        }
    }

    #[derive(Debug)]
    struct ScriptedIo {
        read: io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }

    impl ScriptedIo {
        fn new(read: Vec<u8>) -> Self {
            Self {
                read: io::Cursor::new(read),
                written: Vec::new(),
            }
        }
    }

    impl Read for ScriptedIo {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.read.read(buf)
        }
    }

    impl Write for ScriptedIo {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
