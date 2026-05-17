//! Shared AgentVM payload frame protocol definitions and codecs.
//!
//! The protocol is currently implemented by `docker/guest-payload-server.py`,
//! `vm-frontend/src/payload_client.rs`, and the future Rust guest service.  Keep
//! framing, size limits, and fragmentation behavior here so Tokio read/write
//! integrations can share the same tested compatibility semantics.

use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::io;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_PAYLOAD: usize = 16 * 1024 * 1024;
pub const FRAME_HEADER_LEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameKind(u8);

impl FrameKind {
    pub const PING: Self = Self(b'P');
    pub const RUN_PRIMARY: Self = Self(b'R');
    pub const RUN_DIAGNOSTIC: Self = Self(b'D');
    pub const OK: Self = Self(b'K');
    pub const OUTPUT: Self = Self(b'O');
    pub const EXIT: Self = Self(b'X');
    pub const FAILURE: Self = Self(b'F');
    pub const INPUT: Self = Self(b'I');
    pub const RESIZE: Self = Self(b'W');
    pub const SIGNAL: Self = Self(b'S');

    pub const fn from_byte(byte: u8) -> Self {
        Self(byte)
    }

    pub const fn as_byte(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitFrame {
    pub exit_code: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalFrame {
    pub signal: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResizeFrame {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadEvent {
    Output(Vec<u8>),
    Exit(i32),
    Failure(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: FrameKind, payload: impl Into<Vec<u8>>) -> Result<Self, FrameError> {
        let payload = payload.into();
        if payload.len() > MAX_FRAME_PAYLOAD {
            return Err(FrameError::PayloadTooLarge {
                length: payload.len(),
                max: MAX_FRAME_PAYLOAD,
            });
        }
        Ok(Self { kind, payload })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(FRAME_HEADER_LEN + self.payload.len());
        encoded.push(self.kind.as_byte());
        encoded.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&self.payload);
        encoded
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    PayloadTooLarge { length: usize, max: usize },
    IncompleteFrame { buffered: usize, needed: usize },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge { length, max } => {
                write!(f, "payload frame too large: {length} > {max}")
            }
            Self::IncompleteFrame { buffered, needed } => {
                write!(
                    f,
                    "incomplete payload frame: buffered {buffered} < needed {needed}"
                )
            }
        }
    }
}

impl Error for FrameError {}

#[derive(Debug)]
pub enum PayloadEventError {
    Json(serde_json::Error),
    UnexpectedFrameKind(FrameKind),
}

impl fmt::Display for PayloadEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "payload event JSON failed: {error}"),
            Self::UnexpectedFrameKind(kind) => {
                write!(f, "unexpected payload frame type {:?}", kind.as_byte())
            }
        }
    }
}

impl Error for PayloadEventError {}

#[derive(Debug)]
pub enum AsyncFrameError {
    Io(io::Error),
    Frame(FrameError),
}

impl fmt::Display for AsyncFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "payload frame IO failed: {error}"),
            Self::Frame(error) => write!(f, "{error}"),
        }
    }
}

impl Error for AsyncFrameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(error) => Some(error),
        }
    }
}

impl From<io::Error> for AsyncFrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<FrameError> for AsyncFrameError {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: VecDeque<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, FrameError> {
        self.buffer.extend(bytes.iter().copied());
        let mut frames = Vec::new();
        loop {
            let Some((kind, length)) = self.peek_header()? else {
                break;
            };
            let frame_len = FRAME_HEADER_LEN + length;
            if self.buffer.len() < frame_len {
                break;
            }
            self.buffer.drain(..FRAME_HEADER_LEN);
            let payload: Vec<u8> = self.buffer.drain(..length).collect();
            frames.push(Frame { kind, payload });
        }
        Ok(frames)
    }

    pub fn finish(&self) -> Result<(), FrameError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if self.buffer.len() < FRAME_HEADER_LEN {
            return Err(FrameError::IncompleteFrame {
                buffered: self.buffer.len(),
                needed: FRAME_HEADER_LEN,
            });
        }
        let (_, length) = self.peek_header()?.expect("header length checked");
        Err(FrameError::IncompleteFrame {
            buffered: self.buffer.len(),
            needed: FRAME_HEADER_LEN + length,
        })
    }

    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    fn peek_header(&self) -> Result<Option<(FrameKind, usize)>, FrameError> {
        if self.buffer.len() < FRAME_HEADER_LEN {
            return Ok(None);
        }
        let header = [
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
            self.buffer[4],
        ];
        decode_header(&header).map(Some)
    }
}

pub fn decode_header(header: &[u8; FRAME_HEADER_LEN]) -> Result<(FrameKind, usize), FrameError> {
    let kind = FrameKind::from_byte(header[0]);
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if length > MAX_FRAME_PAYLOAD {
        return Err(FrameError::PayloadTooLarge {
            length,
            max: MAX_FRAME_PAYLOAD,
        });
    }
    Ok((kind, length))
}

pub fn encode_frame(kind: FrameKind, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    Ok(Frame::new(kind, payload.to_vec())?.encode())
}

pub fn payload_event_from_frame(frame: Frame) -> Result<PayloadEvent, PayloadEventError> {
    match frame.kind {
        FrameKind::OUTPUT => Ok(PayloadEvent::Output(frame.payload)),
        FrameKind::EXIT => Ok(PayloadEvent::Exit(
            serde_json::from_slice::<ExitFrame>(&frame.payload)
                .map_err(PayloadEventError::Json)?
                .exit_code,
        )),
        FrameKind::FAILURE => Ok(PayloadEvent::Failure(
            String::from_utf8_lossy(&frame.payload).to_string(),
        )),
        other => Err(PayloadEventError::UnexpectedFrameKind(other)),
    }
}

pub fn signal_payload(signal: i32) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&SignalFrame { signal })
}

pub fn resize_payload(rows: u16, cols: u16) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&ResizeFrame { rows, cols })
}

pub async fn read_frame_async<R>(reader: &mut R) -> Result<Frame, AsyncFrameError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0; FRAME_HEADER_LEN];
    reader.read_exact(&mut header).await?;
    let (kind, length) = decode_header(&header)?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(Frame { kind, payload })
}

pub async fn write_frame_async<W>(writer: &mut W, frame: &Frame) -> Result<(), AsyncFrameError>
where
    W: AsyncWrite + Unpin,
{
    if frame.payload.len() > MAX_FRAME_PAYLOAD {
        return Err(FrameError::PayloadTooLarge {
            length: frame.payload.len(),
            max: MAX_FRAME_PAYLOAD,
        }
        .into());
    }
    let encoded = frame.encode();
    writer.write_all(&encoded).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(chunks: &[&[u8]]) -> Result<Vec<Frame>, FrameError> {
        let mut decoder = FrameDecoder::new();
        let mut frames = Vec::new();
        for chunk in chunks {
            frames.extend(decoder.push(chunk)?);
        }
        decoder.finish()?;
        Ok(frames)
    }

    #[test]
    fn encoded_header_matches_python_network_byte_order() {
        let encoded = encode_frame(FrameKind::PING, b"ok").unwrap();
        assert_eq!(encoded, b"P\0\0\0\x02ok");
    }

    #[test]
    fn fragmented_frame_is_reassembled() {
        let encoded = encode_frame(FrameKind::RUN_PRIMARY, br#"{"script":"true"}"#).unwrap();
        let frames =
            decode_all(&[&encoded[..1], &encoded[1..3], &encoded[3..8], &encoded[8..]]).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].kind, FrameKind::RUN_PRIMARY);
        assert_eq!(frames[0].payload, br#"{"script":"true"}"#);
    }

    #[test]
    fn multiple_frames_decode_in_order() {
        let mut bytes = encode_frame(FrameKind::PING, b"").unwrap();
        bytes.extend(encode_frame(FrameKind::OK, b"ok").unwrap());
        bytes.extend(encode_frame(FrameKind::OUTPUT, b"hello").unwrap());

        let frames = decode_all(&[&bytes]).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].kind, FrameKind::PING);
        assert_eq!(frames[0].payload, b"");
        assert_eq!(frames[1].kind, FrameKind::OK);
        assert_eq!(frames[1].payload, b"ok");
        assert_eq!(frames[2].kind, FrameKind::OUTPUT);
        assert_eq!(frames[2].payload, b"hello");
    }

    #[test]
    fn oversized_encode_is_rejected() {
        let payload = vec![0_u8; MAX_FRAME_PAYLOAD + 1];
        assert_eq!(
            Frame::new(FrameKind::OUTPUT, payload).unwrap_err(),
            FrameError::PayloadTooLarge {
                length: MAX_FRAME_PAYLOAD + 1,
                max: MAX_FRAME_PAYLOAD,
            }
        );
    }

    #[test]
    fn oversized_declared_length_is_rejected_before_payload_allocation() {
        let mut decoder = FrameDecoder::new();
        let mut header = vec![FrameKind::OUTPUT.as_byte()];
        header.extend_from_slice(&((MAX_FRAME_PAYLOAD as u32) + 1).to_be_bytes());

        assert_eq!(
            decoder.push(&header).unwrap_err(),
            FrameError::PayloadTooLarge {
                length: MAX_FRAME_PAYLOAD + 1,
                max: MAX_FRAME_PAYLOAD,
            }
        );
        assert_eq!(decoder.buffered_len(), FRAME_HEADER_LEN);
    }

    #[test]
    fn eof_mid_header_reports_incomplete_frame() {
        let mut decoder = FrameDecoder::new();
        assert!(decoder.push(b"R\0").unwrap().is_empty());
        assert_eq!(
            decoder.finish().unwrap_err(),
            FrameError::IncompleteFrame {
                buffered: 2,
                needed: FRAME_HEADER_LEN,
            }
        );
    }

    #[test]
    fn eof_mid_payload_reports_incomplete_frame() {
        let mut decoder = FrameDecoder::new();
        assert!(decoder.push(b"D\0\0\0\x04xy").unwrap().is_empty());
        assert_eq!(
            decoder.finish().unwrap_err(),
            FrameError::IncompleteFrame {
                buffered: 7,
                needed: 9,
            }
        );
    }

    #[test]
    fn round_trip_survives_every_single_split_point() {
        let original = Frame::new(FrameKind::SIGNAL, br#"{"signal":2}"#.to_vec()).unwrap();
        let encoded = original.encode();
        for split_at in 0..=encoded.len() {
            let decoded = decode_all(&[&encoded[..split_at], &encoded[split_at..]]).unwrap();
            assert_eq!(decoded, vec![original.clone()], "split_at={split_at}");
        }
    }

    #[test]
    fn seeded_arbitrary_inputs_never_panic_or_overallocate() {
        let mut seed = 0x6775657374737663_u64;
        for _case in 0..512 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = (seed as usize) % 96;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                seed = seed
                    .wrapping_mul(2862933555777941757)
                    .wrapping_add(3037000493);
                bytes.push((seed >> 32) as u8);
            }

            let result = std::panic::catch_unwind(|| {
                let mut decoder = FrameDecoder::new();
                let _ = decoder.push(&bytes);
                let _ = decoder.finish();
                decoder.buffered_len()
            });
            assert!(result.is_ok(), "decoder panicked for bytes={bytes:?}");
            assert!(result.unwrap() <= bytes.len());
        }
    }

    #[test]
    fn request_defaults_and_control_payloads_match_wire_schema() {
        let request = PayloadRequest::new("printf ok");
        let request_json = serde_json::to_value(&request).unwrap();
        assert_eq!(request_json["script"], "printf ok");
        assert_eq!(request_json["cwd"], "/");
        assert_eq!(request_json["rows"], 24);
        assert_eq!(request_json["cols"], 80);

        let diagnostic = DiagnosticRequest::new("diag");
        let diagnostic_json = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(diagnostic_json["timeout_seconds"], 10);
        assert_eq!(diagnostic_json["max_output_bytes"], 1024 * 1024);

        assert_eq!(
            serde_json::from_slice::<SignalFrame>(&signal_payload(2).unwrap()).unwrap(),
            SignalFrame { signal: 2 }
        );
        assert_eq!(
            serde_json::from_slice::<ResizeFrame>(&resize_payload(40, 120).unwrap()).unwrap(),
            ResizeFrame {
                rows: 40,
                cols: 120
            }
        );
    }

    #[test]
    fn payload_events_decode_from_typed_frames() {
        assert_eq!(
            payload_event_from_frame(Frame::new(FrameKind::OUTPUT, b"hello".to_vec()).unwrap())
                .unwrap(),
            PayloadEvent::Output(b"hello".to_vec())
        );
        assert_eq!(
            payload_event_from_frame(
                Frame::new(FrameKind::EXIT, br#"{"exit_code":7}"#.to_vec()).unwrap()
            )
            .unwrap(),
            PayloadEvent::Exit(7)
        );
        assert_eq!(
            payload_event_from_frame(Frame::new(FrameKind::FAILURE, b"bad".to_vec()).unwrap())
                .unwrap(),
            PayloadEvent::Failure("bad".to_string())
        );
        assert!(matches!(
            payload_event_from_frame(Frame::new(FrameKind::PING, Vec::new()).unwrap()),
            Err(PayloadEventError::UnexpectedFrameKind(kind)) if kind == FrameKind::PING
        ));
    }

    #[tokio::test]
    async fn async_frame_helpers_round_trip_and_reject_oversized_headers() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let frame = Frame::new(FrameKind::INPUT, b"abc".to_vec()).unwrap();
        write_frame_async(&mut client, &frame).await.unwrap();
        assert_eq!(read_frame_async(&mut server).await.unwrap(), frame);

        let (mut client, mut server) = tokio::io::duplex(64);
        client
            .write_all(&[FrameKind::OUTPUT.as_byte()])
            .await
            .unwrap();
        client
            .write_all(&((MAX_FRAME_PAYLOAD as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(matches!(
            read_frame_async(&mut server).await.unwrap_err(),
            AsyncFrameError::Frame(FrameError::PayloadTooLarge { length, max })
                if length == MAX_FRAME_PAYLOAD + 1 && max == MAX_FRAME_PAYLOAD
        ));
    }
}
