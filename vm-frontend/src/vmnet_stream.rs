use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_FRAME_LEN: u32 = 65_535;
const ETHERNET_LINKTYPE: u32 = 1;

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
    file: File,
    snaplen: u32,
}

impl PcapWriter {
    pub fn create(path: impl AsRef<Path>, snaplen: u32) -> io::Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(path)?;
        write_pcap_global_header(&mut file, snaplen)?;
        Ok(Self { file, snaplen })
    }

    pub fn write_ethernet_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        let captured_len = frame.len().min(self.snaplen as usize);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        self.file.write_all(&(now.as_secs() as u32).to_le_bytes())?;
        self.file.write_all(&now.subsec_micros().to_le_bytes())?;
        self.file.write_all(&(captured_len as u32).to_le_bytes())?;
        self.file.write_all(&(frame.len() as u32).to_le_bytes())?;
        self.file.write_all(&frame[..captured_len])?;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn write_pcap_global_header(writer: &mut impl Write, snaplen: u32) -> io::Result<()> {
    writer.write_all(&0xa1b2c3d4_u32.to_le_bytes())?;
    writer.write_all(&2_u16.to_le_bytes())?;
    writer.write_all(&4_u16.to_le_bytes())?;
    writer.write_all(&0_i32.to_le_bytes())?;
    writer.write_all(&0_u32.to_le_bytes())?;
    writer.write_all(&snaplen.to_le_bytes())?;
    writer.write_all(&ETHERNET_LINKTYPE.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
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
            ScriptedReadWrite::new(vec![
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
            ScriptedReadWrite::new(vec![ReadStep::Bytes(encoded), ReadStep::WouldBlock]),
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
    fn writes_standard_pcap_file() {
        let path = std::env::temp_dir().join(format!(
            "agentvm-vmnet-test-{}-{}.pcap",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let frame = ethernet_frame();

        let mut writer = PcapWriter::create(&path, 65_535).expect("create pcap");
        writer
            .write_ethernet_frame(&frame)
            .expect("write pcap frame");
        writer.flush().expect("flush");

        let bytes = fs::read(&path).expect("read pcap");
        let _ = fs::remove_file(&path);
        assert_eq!(&bytes[..4], &0xa1b2c3d4_u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_le_bytes(bytes[32..36].try_into().unwrap()),
            frame.len() as u32
        );
        assert_eq!(&bytes[40..], frame.as_slice());
    }

    #[derive(Debug)]
    enum ReadStep {
        Bytes(Vec<u8>),
        WouldBlock,
    }

    #[derive(Debug)]
    struct ScriptedReadWrite {
        steps: VecDeque<ReadStep>,
        written: Vec<u8>,
    }

    impl ScriptedReadWrite {
        fn new(steps: Vec<ReadStep>) -> Self {
            Self {
                steps: steps.into(),
                written: Vec::new(),
            }
        }
    }

    impl Read for ScriptedReadWrite {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.steps.pop_front() {
                Some(ReadStep::Bytes(bytes)) => {
                    let count = bytes.len().min(buf.len());
                    buf[..count].copy_from_slice(&bytes[..count]);
                    if count < bytes.len() {
                        self.steps
                            .push_front(ReadStep::Bytes(bytes[count..].to_vec()));
                    }
                    Ok(count)
                }
                Some(ReadStep::WouldBlock) => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                None => Ok(0),
            }
        }
    }

    impl Write for ScriptedReadWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
