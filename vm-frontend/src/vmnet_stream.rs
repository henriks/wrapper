use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pcap_file::pcap::{PcapHeader, PcapPacket, PcapWriter as RawPcapWriter};
use pcap_file::{DataLink, Endianness};

pub const DEFAULT_MAX_FRAME_LEN: u32 = 65_535;

#[derive(Debug)]
pub struct VmnetStreamEndpoint {
    socket_path: PathBuf,
    listener: UnixListener,
}

impl VmnetStreamEndpoint {
    pub fn bind(socket_path: impl Into<PathBuf>) -> io::Result<Self> {
        let socket_path = socket_path.into();
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::remove_file(&socket_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&socket_path)?;
        Ok(Self {
            socket_path,
            listener,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn accept_one(&self) -> io::Result<VmnetFrameIo> {
        let (stream, _addr) = self.listener.accept()?;
        Ok(QemuFrameIo::new(stream, DEFAULT_MAX_FRAME_LEN))
    }

    pub fn accept_one_nonblocking(&self) -> io::Result<VmnetFrameIo> {
        let (stream, _addr) = self.listener.accept()?;
        stream.set_nonblocking(true)?;
        Ok(QemuFrameIo::new(stream, DEFAULT_MAX_FRAME_LEN))
    }

    pub fn listener_raw_fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.listener.set_nonblocking(nonblocking)
    }

    pub fn accept_ready(&self) -> io::Result<Option<VmnetFrameIo>> {
        match self.listener.accept() {
            Ok((stream, _addr)) => {
                stream.set_nonblocking(true)?;
                Ok(Some(QemuFrameIo::new(stream, DEFAULT_MAX_FRAME_LEN)))
            }
            Err(error) if would_block(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug)]
pub struct QemuFrameIo<T> {
    stream: T,
    max_frame_len: u32,
    read_buf: Vec<u8>,
}

pub type VmnetFrameIo = QemuFrameIo<UnixStream>;

impl<T> QemuFrameIo<T>
where
    T: Read + Write,
{
    pub fn new(stream: T, max_frame_len: u32) -> Self {
        Self {
            stream,
            max_frame_len,
            read_buf: Vec::new(),
        }
    }

    pub fn read_frame(&mut self) -> Result<Option<Vec<u8>>, VmnetStreamError> {
        let Some(length_bytes) = read_exact_or_eof(&mut self.stream, 4)? else {
            return Ok(None);
        };
        let length = u32::from_be_bytes(length_bytes.try_into().expect("length is four bytes"));
        if length == 0 || length > self.max_frame_len {
            return Err(VmnetStreamError::InvalidFrameLength {
                length,
                max: self.max_frame_len,
            });
        }

        let mut frame = vec![0; length as usize];
        read_payload_exact(&mut self.stream, &mut frame)?;
        Ok(Some(frame))
    }

    pub fn try_read_frame(&mut self) -> Result<FrameRead, VmnetStreamError> {
        if let Some(frame) = self.pop_buffered_frame()? {
            return Ok(FrameRead::Frame(frame));
        }

        let mut chunk = [0; 8192];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) if self.read_buf.is_empty() => return Ok(FrameRead::Eof),
                Ok(0) => return Err(VmnetStreamError::TruncatedFrame),
                Ok(count) => {
                    self.read_buf.extend_from_slice(&chunk[..count]);
                    if let Some(frame) = self.pop_buffered_frame()? {
                        return Ok(FrameRead::Frame(frame));
                    }
                }
                Err(error) if would_block(&error) => return Ok(FrameRead::WouldBlock),
                Err(error) => return Err(VmnetStreamError::Io(error)),
            }
        }
    }

    fn pop_buffered_frame(&mut self) -> Result<Option<Vec<u8>>, VmnetStreamError> {
        if self.read_buf.len() < 4 {
            return Ok(None);
        }
        let length = u32::from_be_bytes(
            self.read_buf[0..4]
                .try_into()
                .expect("length prefix is four bytes"),
        );
        if length == 0 || length > self.max_frame_len {
            return Err(VmnetStreamError::InvalidFrameLength {
                length,
                max: self.max_frame_len,
            });
        }
        let frame_end = 4 + length as usize;
        if self.read_buf.len() < frame_end {
            return Ok(None);
        }
        let frame = self.read_buf[4..frame_end].to_vec();
        self.read_buf.drain(..frame_end);
        Ok(Some(frame))
    }

    pub fn write_frame(&mut self, frame: &[u8]) -> Result<(), VmnetStreamError> {
        let length = u32::try_from(frame.len()).map_err(|_| VmnetStreamError::FrameTooLarge {
            length: usize::MAX,
            max: self.max_frame_len,
        })?;
        if length == 0 || length > self.max_frame_len {
            return Err(VmnetStreamError::FrameTooLarge {
                length: frame.len(),
                max: self.max_frame_len,
            });
        }
        self.stream.write_all(&length.to_be_bytes())?;
        self.stream.write_all(frame)?;
        Ok(())
    }

    pub fn into_inner(self) -> T {
        self.stream
    }
}

impl<T> QemuFrameIo<T>
where
    T: AsRawFd,
{
    pub fn raw_fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameRead {
    Frame(Vec<u8>),
    WouldBlock,
    Eof,
}

#[derive(Debug)]
pub enum VmnetStreamError {
    Io(io::Error),
    TruncatedLength { received: usize },
    TruncatedFrame,
    InvalidFrameLength { length: u32, max: u32 },
    FrameTooLarge { length: usize, max: u32 },
}

impl From<io::Error> for VmnetStreamError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn would_block(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

fn read_exact_or_eof(
    reader: &mut impl Read,
    length: usize,
) -> Result<Option<Vec<u8>>, VmnetStreamError> {
    let mut buf = vec![0; length];
    let mut received = 0;
    while received < length {
        let read = reader.read(&mut buf[received..])?;
        if read == 0 {
            if received == 0 {
                return Ok(None);
            }
            return Err(VmnetStreamError::TruncatedLength { received });
        }
        received += read;
    }
    Ok(Some(buf))
}

fn read_payload_exact(reader: &mut impl Read, frame: &mut [u8]) -> Result<(), VmnetStreamError> {
    let mut received = 0;
    while received < frame.len() {
        let read = reader.read(&mut frame[received..])?;
        if read == 0 {
            return Err(VmnetStreamError::TruncatedFrame);
        }
        received += read;
    }
    Ok(())
}

#[derive(Debug)]
pub struct PcapWriter {
    writer: RawPcapWriter<File>,
    snaplen: u32,
}

impl PcapWriter {
    pub fn create(path: impl AsRef<Path>, snaplen: u32) -> io::Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        let writer = RawPcapWriter::with_header(
            file,
            PcapHeader {
                snaplen,
                datalink: DataLink::ETHERNET,
                endianness: Endianness::native(),
                ..Default::default()
            },
        )
        .map_err(pcap_error)?;
        Ok(Self { writer, snaplen })
    }

    pub fn write_ethernet_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        let captured_len = frame.len().min(self.snaplen as usize);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let packet = PcapPacket::new(now, frame.len() as u32, &frame[..captured_len]);
        self.writer.write_packet(&packet).map_err(pcap_error)?;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn pcap_error(error: pcap_file::PcapError) -> io::Error {
    io::Error::new(ErrorKind::Other, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ReadStep, ScriptedStream};
    use proptest::prelude::*;
    use std::io::Cursor;

    fn ethernet_frame() -> Vec<u8> {
        vec![
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02, 0xfc, 0x12, 0x34, 0x56, 0x78, 0x08, 0x06, 0,
            1, 8, 0,
        ]
    }

    #[test]
    fn decodes_big_endian_qemu_frame() {
        let frame = ethernet_frame();
        let mut input = Vec::new();
        input.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        input.extend_from_slice(&frame);

        let mut io = QemuFrameIo::new(Cursor::new(input), DEFAULT_MAX_FRAME_LEN);
        assert_eq!(io.read_frame().expect("read frame"), Some(frame));
    }

    #[test]
    fn encodes_big_endian_qemu_frame() {
        let frame = ethernet_frame();

        let mut io = QemuFrameIo::new(Cursor::new(Vec::new()), DEFAULT_MAX_FRAME_LEN);
        io.write_frame(&frame).expect("write frame");

        let bytes = io.into_inner().into_inner();
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into().unwrap()),
            frame.len() as u32
        );
        assert_eq!(&bytes[4..], frame.as_slice());
    }

    #[test]
    fn nonblocking_reader_preserves_partial_frame_across_would_block() {
        let frame = ethernet_frame();
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&frame);

        let first = encoded[..6].to_vec();
        let second = encoded[6..].to_vec();
        let mut io = QemuFrameIo::new(
            ScriptedStream::new([
                ReadStep::Bytes(first),
                ReadStep::WouldBlock,
                ReadStep::Bytes(second),
            ]),
            DEFAULT_MAX_FRAME_LEN,
        );

        assert_eq!(io.try_read_frame().expect("first"), FrameRead::WouldBlock);
        assert_eq!(
            io.try_read_frame().expect("second"),
            FrameRead::Frame(frame)
        );
    }

    #[test]
    fn nonblocking_reader_leaves_following_frame_buffered() {
        let first = ethernet_frame();
        let second = vec![0xaa, 0xbb, 0xcc, 0xdd];
        let mut encoded = Vec::new();
        for frame in [&first, &second] {
            encoded.extend_from_slice(&(frame.len() as u32).to_be_bytes());
            encoded.extend_from_slice(frame);
        }
        let mut io = QemuFrameIo::new(
            ScriptedStream::new([ReadStep::Bytes(encoded), ReadStep::WouldBlock]),
            DEFAULT_MAX_FRAME_LEN,
        );

        assert_eq!(io.try_read_frame().expect("first"), FrameRead::Frame(first));
        assert_eq!(
            io.try_read_frame().expect("second"),
            FrameRead::Frame(second)
        );
        assert_eq!(io.try_read_frame().expect("blocked"), FrameRead::WouldBlock);
    }

    #[test]
    fn rejects_truncated_payload_without_panic() {
        let mut input = Vec::new();
        input.extend_from_slice(&4_u32.to_be_bytes());
        input.extend_from_slice(&[1, 2]);

        let mut io = QemuFrameIo::new(Cursor::new(input), DEFAULT_MAX_FRAME_LEN);
        assert!(matches!(
            io.read_frame(),
            Err(VmnetStreamError::TruncatedFrame)
        ));
    }

    #[test]
    fn rejects_truncated_length_without_treating_as_clean_eof() {
        let mut io = QemuFrameIo::new(Cursor::new(vec![0, 0]), DEFAULT_MAX_FRAME_LEN);

        assert!(matches!(
            io.read_frame(),
            Err(VmnetStreamError::TruncatedLength { received: 2 })
        ));
    }

    #[test]
    fn rejects_zero_length_frames_on_read_and_write() {
        let mut reader = QemuFrameIo::new(Cursor::new(0_u32.to_be_bytes().to_vec()), 1500);

        assert!(matches!(
            reader.read_frame(),
            Err(VmnetStreamError::InvalidFrameLength {
                length: 0,
                max: 1500
            })
        ));

        let mut writer = QemuFrameIo::new(Cursor::new(Vec::new()), 1500);
        assert!(matches!(
            writer.write_frame(&[]),
            Err(VmnetStreamError::FrameTooLarge {
                length: 0,
                max: 1500
            })
        ));
    }

    #[test]
    fn rejects_oversized_frame_before_allocation() {
        let input = Cursor::new(9000_u32.to_be_bytes().to_vec());

        let mut io = QemuFrameIo::new(input, 1500);
        assert!(matches!(
            io.read_frame(),
            Err(VmnetStreamError::InvalidFrameLength {
                length: 9000,
                max: 1500
            })
        ));
    }

    #[test]
    fn reads_max_sized_frame_without_rejecting_boundary() {
        let frame = vec![0x5a; 64];
        let mut input = Vec::new();
        input.extend_from_slice(&64_u32.to_be_bytes());
        input.extend_from_slice(&frame);

        let mut io = QemuFrameIo::new(Cursor::new(input), 64);
        assert_eq!(io.read_frame().expect("max frame"), Some(frame));
    }

    #[test]
    fn writes_standard_pcap_file() {
        let file = tempfile::Builder::new()
            .prefix("agentvm-vmnet-test-")
            .suffix(".pcap")
            .tempfile()
            .expect("pcap temp file");
        let path = file.path().to_path_buf();
        let frame = ethernet_frame();

        let mut writer = PcapWriter::create(&path, 65_535).expect("create pcap");
        writer
            .write_ethernet_frame(&frame)
            .expect("write pcap frame");
        writer.flush().expect("flush");

        let bytes = fs::read(&path).expect("read pcap");
        assert_eq!(&bytes[..4], &0xa1b2c3d4_u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_le_bytes(bytes[32..36].try_into().unwrap()),
            frame.len() as u32
        );
        assert_eq!(&bytes[40..], frame.as_slice());
    }

    fn chunk_steps(bytes: &[u8], chunk_sizes: &[usize]) -> Vec<ReadStep> {
        let mut steps = Vec::new();
        let mut offset = 0;
        let mut size_index = 0;
        while offset < bytes.len() {
            let remaining = bytes.len() - offset;
            let chunk_size = chunk_sizes
                .get(size_index)
                .copied()
                .unwrap_or(7)
                .clamp(1, remaining);
            steps.push(ReadStep::Bytes(bytes[offset..offset + chunk_size].to_vec()));
            steps.push(ReadStep::WouldBlock);
            offset += chunk_size;
            size_index += 1;
        }
        steps
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 96,
            max_shrink_iters: 1024,
            ..ProptestConfig::default()
        })]

        #[test]
        fn proptest_valid_chunked_frames_preserve_boundaries(
            frames in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..=64), 1..=16),
            chunk_sizes in prop::collection::vec(1usize..=11, 1..=96),
        ) {
            let frame_refs = frames.iter().map(Vec::as_slice).collect::<Vec<_>>();
            let encoded = crate::test_support::qemu_stream_bytes(&frame_refs);
            let mut io = QemuFrameIo::new(ScriptedStream::new(chunk_steps(&encoded, &chunk_sizes)), 64);
            let mut actual = Vec::new();
            let mut polls = 0usize;

            while actual.len() < frames.len() {
                polls += 1;
                prop_assert!(polls <= encoded.len() + frames.len() + 8);
                match io.try_read_frame().expect("valid chunked frame stream") {
                    FrameRead::Frame(frame) => actual.push(frame),
                    FrameRead::WouldBlock => {}
                    FrameRead::Eof => break,
                }
            }

            prop_assert_eq!(actual, frames);
        }

        #[test]
        fn proptest_read_frame_arbitrary_bytes_stays_bounded(bytes in prop::collection::vec(any::<u8>(), 0..=200)) {
            let mut io = QemuFrameIo::new(Cursor::new(bytes), 64);
            match io.read_frame() {
                Ok(Some(frame)) => prop_assert!((1..=64).contains(&frame.len())),
                Ok(None) => {}
                Err(VmnetStreamError::InvalidFrameLength { length, max }) => {
                    prop_assert!(length == 0 || length > max);
                    prop_assert_eq!(max, 64);
                }
                Err(VmnetStreamError::TruncatedLength { received }) => {
                    prop_assert!((1..4).contains(&received));
                }
                Err(VmnetStreamError::TruncatedFrame) => {}
                Err(VmnetStreamError::FrameTooLarge { .. }) => {
                    prop_assert!(false, "read_frame should report invalid length, not write-side frame too large");
                }
                Err(VmnetStreamError::Io(error)) => {
                    prop_assert!(false, "cursor read should not fail: {error}");
                }
            }
        }

        #[test]
        fn proptest_invalid_lengths_are_rejected_before_payload_read(
            length in prop_oneof![Just(0_u32), 65_u32..=u32::MAX],
        ) {
            let mut io = QemuFrameIo::new(Cursor::new(length.to_be_bytes().to_vec()), 64);
            match io.read_frame() {
                Err(VmnetStreamError::InvalidFrameLength { length: actual, max }) => {
                    prop_assert_eq!(actual, length);
                    prop_assert_eq!(max, 64);
                }
                other => prop_assert!(false, "invalid length produced unexpected result: {other:?}"),
            }
        }

        #[test]
        fn proptest_write_frame_round_trips_length_prefix(frame in prop::collection::vec(any::<u8>(), 1..=64)) {
            let mut io = QemuFrameIo::new(Cursor::new(Vec::new()), 64);
            io.write_frame(&frame).expect("write generated frame");

            let bytes = io.into_inner().into_inner();
            prop_assert_eq!(u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize, frame.len());
            prop_assert_eq!(&bytes[4..], frame.as_slice());
        }

        #[test]
        fn proptest_write_frame_rejects_empty_and_oversized(
            frame in prop_oneof![
                Just(Vec::new()),
                prop::collection::vec(any::<u8>(), 65..=160),
            ],
        ) {
            let mut io = QemuFrameIo::new(Cursor::new(Vec::new()), 64);
            match io.write_frame(&frame) {
                Err(VmnetStreamError::FrameTooLarge { length, max }) => {
                    prop_assert_eq!(length, frame.len());
                    prop_assert_eq!(max, 64);
                }
                other => prop_assert!(false, "invalid generated write frame produced unexpected result: {other:?}"),
            }
        }
    }

    #[test]
    #[ignore = "property stress: run explicitly with `cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_stream::tests::stress_many_chunked_frame_splits -- --ignored --nocapture`"]
    fn stress_many_chunked_frame_splits() {
        let frames = (1..=128)
            .map(|len| vec![len as u8; len])
            .collect::<Vec<_>>();
        let frame_refs = frames.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let encoded = crate::test_support::qemu_stream_bytes(&frame_refs);

        for split in 1..=31 {
            let chunk_sizes = vec![split; encoded.len().div_ceil(split)];
            let mut io = QemuFrameIo::new(
                ScriptedStream::new(chunk_steps(&encoded, &chunk_sizes)),
                128,
            );
            for expected in &frames {
                loop {
                    match io.try_read_frame().expect("stress chunked stream") {
                        FrameRead::Frame(actual) => {
                            assert_eq!(&actual, expected);
                            break;
                        }
                        FrameRead::WouldBlock => {}
                        FrameRead::Eof => panic!("unexpected eof for split {split}"),
                    }
                }
            }
        }
    }
}
