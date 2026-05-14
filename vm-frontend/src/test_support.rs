#![allow(dead_code)]

use std::collections::VecDeque;
use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hickory_proto::op::{Message, Query};
use hickory_proto::rr::{Name, RecordType};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol,
    Ipv4Address, Ipv4Packet, Ipv4Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
};

use crate::l2_gateway::ipv4_checksum;
use crate::launch::LaunchError;
use crate::tls_mitm::TlsMitmAuthority;
use crate::vmnet_stream::QemuFrameIo;
use crate::{FrontendConfig, GuestNetwork, RuntimePaths};

pub(crate) const TEST_GUEST_MAC: EthernetAddress =
    EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x78]);
pub(crate) const TEST_GATEWAY_MAC: EthernetAddress =
    EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x01]);
pub(crate) const TEST_GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
pub(crate) const TEST_GATEWAY_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
pub(crate) const TEST_DNS_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 3);
pub(crate) const TEST_PUBLIC_IP: Ipv4Address = Ipv4Address::new(93, 184, 216, 34);

pub(crate) struct TestTempDir {
    path: PathBuf,
}

impl TestTempDir {
    pub(crate) fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agentvm-frontend-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test temp dir");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.path.join(path)
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) struct FrontendFixture {
    pub(crate) root: TestTempDir,
    pub(crate) project: PathBuf,
    pub(crate) run_dir: PathBuf,
    pub(crate) artifact_manifest: PathBuf,
    pub(crate) qemu: PathBuf,
}

impl FrontendFixture {
    pub(crate) fn new(name: &str) -> Self {
        let root = TestTempDir::new(name);
        let project = root.join("repo");
        let out = root.join("docker/out");
        fs::create_dir_all(&project).expect("create project");
        fs::create_dir_all(&out).expect("create artifact dir");
        fs::write(out.join("vmlinuz"), b"kernel").expect("write kernel");
        fs::write(out.join("initrd.img"), b"initrd").expect("write initrd");
        fs::write(out.join("rootfs.raw"), b"rootfs").expect("write rootfs");
        let artifact_manifest = out.join("artifact-manifest.json");
        fs::write(
            &artifact_manifest,
            r#"{
              "schema_version": 1,
              "artifacts": {
                "kernel": "docker/out/vmlinuz",
                "initrd": "docker/out/initrd.img",
                "rootfs": "docker/out/rootfs.raw"
              },
              "vm": {
                "cpus": 2,
                "memory_bytes": 2147483648,
                "virtiofs_tag": "agentvm",
                "kernel_cmdline": "console=hvc0 root=/dev/vda"
              }
            }"#,
        )
        .expect("write artifact manifest");
        let run_dir = root.join(".sandbox/docker-vm/run");
        let qemu = PathBuf::from("/usr/bin/qemu-system-x86_64");
        Self {
            root,
            project,
            run_dir,
            artifact_manifest,
            qemu,
        }
    }

    pub(crate) fn frontend_config(&self) -> Result<FrontendConfig, LaunchError> {
        FrontendConfig::from_artifact_manifest_file(
            self.project.clone(),
            self.run_dir.clone(),
            self.qemu.clone(),
            &self.artifact_manifest,
        )
    }

    pub(crate) fn runtime_paths(&self) -> RuntimePaths {
        RuntimePaths::under(self.run_dir.clone())
    }
}

pub(crate) fn default_guest_network() -> GuestNetwork {
    GuestNetwork::default()
}

pub(crate) fn smol_time_ms(milliseconds: i64) -> Instant {
    Instant::from_millis(milliseconds)
}

pub(crate) fn qemu_stream_bytes(frames: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for frame in frames {
        let length = u32::try_from(frame.len())
            .expect("test QEMU frame length must fit in u32")
            .to_be_bytes();
        bytes.extend_from_slice(&length);
        bytes.extend_from_slice(frame);
    }
    bytes
}

pub(crate) fn memory_qemu_frame_io(frames: &[&[u8]]) -> QemuFrameIo<std::io::Cursor<Vec<u8>>> {
    QemuFrameIo::new(std::io::Cursor::new(qemu_stream_bytes(frames)), 65_535)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReadStep {
    Bytes(Vec<u8>),
    WouldBlock,
    TimedOut,
    Error(ErrorKind),
    Eof,
}

#[derive(Debug, Default)]
pub(crate) struct ScriptedStream {
    steps: VecDeque<ReadStep>,
    current: VecDeque<u8>,
    written: Vec<u8>,
}

impl ScriptedStream {
    pub(crate) fn new(steps: impl IntoIterator<Item = ReadStep>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            current: VecDeque::new(),
            written: Vec::new(),
        }
    }

    pub(crate) fn written(&self) -> &[u8] {
        &self.written
    }
}

impl Read for ScriptedStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            if !self.current.is_empty() {
                let count = buf.len().min(self.current.len());
                for target in &mut buf[..count] {
                    *target = self.current.pop_front().expect("current byte");
                }
                return Ok(count);
            }

            match self.steps.pop_front().unwrap_or(ReadStep::Eof) {
                ReadStep::Bytes(bytes) => self.current.extend(bytes),
                ReadStep::WouldBlock => {
                    return Err(io::Error::new(
                        ErrorKind::WouldBlock,
                        "scripted would-block",
                    ));
                }
                ReadStep::TimedOut => {
                    return Err(io::Error::new(ErrorKind::TimedOut, "scripted timeout"));
                }
                ReadStep::Error(kind) => return Err(io::Error::new(kind, "scripted error")),
                ReadStep::Eof => return Ok(0),
            }
        }
    }
}

impl Write for ScriptedStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn udp_frame(
    src_port: u16,
    dst_port: u16,
    src_ip: Ipv4Address,
    dst_ip: Ipv4Address,
    payload: &[u8],
) -> Vec<u8> {
    let udp_len = 8 + payload.len();
    let mut udp = Vec::with_capacity(udp_len);
    udp.extend_from_slice(&src_port.to_be_bytes());
    udp.extend_from_slice(&dst_port.to_be_bytes());
    udp.extend_from_slice(&(udp_len as u16).to_be_bytes());
    udp.extend_from_slice(&0_u16.to_be_bytes());
    udp.extend_from_slice(payload);
    ipv4_ethernet_frame(src_ip, dst_ip, 17, &udp)
}

pub(crate) fn dns_query_frame(domain: &str, dst_ip: Ipv4Address, src_port: u16) -> Vec<u8> {
    let mut message = Message::query();
    message.metadata.id = 0x1234;
    message.add_query(Query::query(
        Name::from_ascii(domain).expect("query name"),
        RecordType::A,
    ));
    let payload = message.to_vec().expect("serialize dns query");
    udp_frame(src_port, 53, TEST_GUEST_IP, dst_ip, &payload)
}

pub(crate) fn malformed_udp_len_frame(declared_udp_len: u16) -> Vec<u8> {
    let mut frame = udp_frame(53000, 1234, TEST_GUEST_IP, TEST_PUBLIC_IP, b"payload");
    frame[38..40].copy_from_slice(&declared_udp_len.to_be_bytes());
    frame
}

pub(crate) fn ipv6_frame(payload: &[u8]) -> Vec<u8> {
    ethernet_frame(0x86dd, payload)
}

pub(crate) fn unknown_ethertype_frame(ethertype: u16, payload: &[u8]) -> Vec<u8> {
    ethernet_frame(ethertype, payload)
}

pub(crate) fn unknown_ipv4_protocol_frame(protocol: u8) -> Vec<u8> {
    ipv4_ethernet_frame(TEST_GUEST_IP, TEST_PUBLIC_IP, protocol, &[])
}

pub(crate) fn tcp_frame(
    src_port: u16,
    dst_port: u16,
    src_ip: Ipv4Address,
    dst_ip: Ipv4Address,
    control: TcpControl,
    seq: TcpSeqNumber,
    ack: Option<TcpSeqNumber>,
    payload: &[u8],
) -> Vec<u8> {
    let tcp = TcpRepr {
        src_port,
        dst_port,
        control,
        seq_number: seq,
        ack_number: ack,
        window_len: 4096,
        window_scale: None,
        max_seg_size: (control == TcpControl::Syn).then_some(1460),
        sack_permitted: false,
        sack_ranges: [None, None, None],
        timestamp: None,
        payload,
    };
    let ipv4 = Ipv4Repr {
        src_addr: src_ip,
        dst_addr: dst_ip,
        next_header: IpProtocol::Tcp,
        payload_len: tcp.buffer_len(),
        hop_limit: 64,
    };
    let ethernet = EthernetRepr {
        src_addr: TEST_GUEST_MAC,
        dst_addr: TEST_GATEWAY_MAC,
        ethertype: EthernetProtocol::Ipv4,
    };

    let mut frame = vec![0; ethernet.buffer_len() + ipv4.buffer_len() + tcp.buffer_len()];
    ethernet.emit(&mut EthernetFrame::new_unchecked(&mut frame));
    ipv4.emit(
        &mut Ipv4Packet::new_unchecked(&mut frame[ethernet.buffer_len()..]),
        &ChecksumCapabilities::default(),
    );
    tcp.emit(
        &mut TcpPacket::new_unchecked(&mut frame[ethernet.buffer_len() + ipv4.buffer_len()..]),
        &IpAddress::Ipv4(src_ip),
        &IpAddress::Ipv4(dst_ip),
        &ChecksumCapabilities::default(),
    );
    frame
}

pub(crate) fn tcp_syn_frame(dst_ip: Ipv4Address, dst_port: u16) -> Vec<u8> {
    tcp_frame(
        49152,
        dst_port,
        TEST_GUEST_IP,
        dst_ip,
        TcpControl::Syn,
        TcpSeqNumber(100),
        None,
        &[],
    )
}

pub(crate) fn arp_reply_frame() -> Vec<u8> {
    let mut frame = Vec::with_capacity(42);
    frame.extend_from_slice(TEST_GATEWAY_MAC.as_bytes());
    frame.extend_from_slice(TEST_GUEST_MAC.as_bytes());
    frame.extend_from_slice(&0x0806u16.to_be_bytes());
    frame.extend_from_slice(&1u16.to_be_bytes());
    frame.extend_from_slice(&0x0800u16.to_be_bytes());
    frame.push(6);
    frame.push(4);
    frame.extend_from_slice(&2u16.to_be_bytes());
    frame.extend_from_slice(TEST_GUEST_MAC.as_bytes());
    frame.extend_from_slice(&TEST_GUEST_IP.octets());
    frame.extend_from_slice(TEST_GATEWAY_MAC.as_bytes());
    frame.extend_from_slice(&TEST_GATEWAY_IP.octets());
    frame
}

pub(crate) fn parse_tcp_frame(frame: &[u8]) -> Option<TcpRepr<'_>> {
    let ethernet = EthernetFrame::new_checked(frame).ok()?;
    let ethernet = EthernetRepr::parse(&ethernet).ok()?;
    if ethernet.ethertype != EthernetProtocol::Ipv4 {
        return None;
    }
    let ip_offset = ethernet.buffer_len();
    let ipv4 = Ipv4Packet::new_checked(&frame[ip_offset..]).ok()?;
    let ipv4 = Ipv4Repr::parse(&ipv4, &ChecksumCapabilities::default()).ok()?;
    if ipv4.next_header != IpProtocol::Tcp {
        return None;
    }
    let tcp_offset = ip_offset + ipv4.buffer_len();
    let tcp = TcpPacket::new_checked(&frame[tcp_offset..]).ok()?;
    TcpRepr::parse(
        &tcp,
        &IpAddress::Ipv4(ipv4.src_addr),
        &IpAddress::Ipv4(ipv4.dst_addr),
        &ChecksumCapabilities::default(),
    )
    .ok()
}

fn ethernet_frame(ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.extend_from_slice(TEST_GATEWAY_MAC.as_bytes());
    frame.extend_from_slice(TEST_GUEST_MAC.as_bytes());
    frame.extend_from_slice(&ethertype.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn ipv4_ethernet_frame(
    src_ip: Ipv4Address,
    dst_ip: Ipv4Address,
    protocol: u8,
    payload: &[u8],
) -> Vec<u8> {
    let ip_total_len = 20 + payload.len();
    let mut ipv4 = Vec::with_capacity(ip_total_len);
    ipv4.push(0x45);
    ipv4.push(0);
    ipv4.extend_from_slice(&(ip_total_len as u16).to_be_bytes());
    ipv4.extend_from_slice(&0_u16.to_be_bytes());
    ipv4.extend_from_slice(&0_u16.to_be_bytes());
    ipv4.push(64);
    ipv4.push(protocol);
    ipv4.extend_from_slice(&0_u16.to_be_bytes());
    ipv4.extend_from_slice(&src_ip.octets());
    ipv4.extend_from_slice(&dst_ip.octets());
    let checksum = ipv4_checksum(&ipv4);
    ipv4[10..12].copy_from_slice(&checksum.to_be_bytes());
    ipv4.extend_from_slice(payload);
    ethernet_frame(0x0800, &ipv4)
}

pub(crate) struct TestCa {
    _dir: TestTempDir,
    pub(crate) cert_pem: String,
    pub(crate) key_pem: String,
    pub(crate) cert_path: PathBuf,
    pub(crate) key_path: PathBuf,
}

impl TestCa {
    pub(crate) fn new(name: &str) -> Self {
        let dir = TestTempDir::new(name);
        let ca_key = KeyPair::generate().expect("generate test CA key");
        let mut params =
            CertificateParams::new(vec!["agentvm-test-ca".to_string()]).expect("CA params");
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, "agentvm-test-ca");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::CrlSign,
        ];
        let ca_cert = params.self_signed(&ca_key).expect("generate test CA cert");
        let cert_pem = ca_cert.pem();
        let key_pem = ca_key.serialize_pem();
        let cert_path = dir.join("mitm-ca.crt");
        let key_path = dir.join("mitm-ca.key");
        fs::write(&cert_path, &cert_pem).expect("write test CA cert");
        fs::write(&key_path, &key_pem).expect("write test CA key");
        Self {
            _dir: dir,
            cert_pem,
            key_pem,
            cert_path,
            key_path,
        }
    }

    pub(crate) fn authority(&self) -> TlsMitmAuthority {
        TlsMitmAuthority::from_pem(&self.cert_pem, &self.key_pem).expect("load test CA")
    }
}

pub(crate) struct StaticTcpServer {
    addr: SocketAddr,
    handle: Option<JoinHandle<()>>,
}

impl StaticTcpServer {
    pub(crate) fn respond_once(response: Vec<u8>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake upstream");
        listener
            .set_nonblocking(true)
            .expect("fake upstream nonblocking");
        let addr = listener.local_addr().expect("fake upstream addr");
        let handle = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _peer)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
        });
        Self {
            addr,
            handle: Some(handle),
        }
    }

    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for StaticTcpServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmnet_stream::FrameRead;
    use smoltcp::wire::TcpControl;

    #[test]
    fn frontend_fixture_loads_config_with_absolute_runtime_paths() {
        let fixture = FrontendFixture::new("config");
        let config = fixture.frontend_config().expect("frontend config");

        assert_eq!(config.project, fixture.project);
        assert_eq!(
            config.runtime.composed_bind_manifest,
            fixture
                .run_dir
                .join("guest-config")
                .join("composed-binds.json")
        );
        assert_eq!(
            fixture.runtime_paths().vmnet_sock,
            fixture.run_dir.join("vmnet.sock")
        );
    }

    #[test]
    fn memory_qemu_frame_fixture_decodes_multiple_frames() {
        let mut io = memory_qemu_frame_io(&[b"one", b"two"]);

        assert_eq!(
            io.try_read_frame().expect("first frame"),
            FrameRead::Frame(b"one".to_vec())
        );
        assert_eq!(
            io.try_read_frame().expect("second frame"),
            FrameRead::Frame(b"two".to_vec())
        );
    }

    #[test]
    fn test_ca_loads_as_mitm_authority() {
        let ca = TestCa::new("ca");
        let authority = ca.authority();
        authority
            .generate_server_certificate("example.com")
            .expect("server cert");
        assert!(ca.cert_path.exists());
        assert!(ca.key_path.exists());
    }

    #[test]
    fn scripted_stream_preserves_chunks_and_writes() {
        let mut stream = ScriptedStream::new([
            ReadStep::Bytes(b"ab".to_vec()),
            ReadStep::WouldBlock,
            ReadStep::Bytes(b"cd".to_vec()),
        ]);
        let mut buf = [0_u8; 4];

        assert_eq!(stream.read(&mut buf).expect("first read"), 2);
        assert_eq!(&buf[..2], b"ab");
        assert_eq!(
            stream.read(&mut buf).expect_err("would-block").kind(),
            ErrorKind::WouldBlock
        );
        assert_eq!(stream.read(&mut buf).expect("second read"), 2);
        assert_eq!(&buf[..2], b"cd");
        assert_eq!(stream.read(&mut buf).expect("eof"), 0);

        stream.write_all(b"out").expect("write");
        assert_eq!(stream.written(), b"out");
    }

    #[test]
    fn packet_builders_emit_parseable_tcp_syn_and_udp_frame() {
        let tcp = tcp_syn_frame(TEST_PUBLIC_IP, 443);
        let parsed = parse_tcp_frame(&tcp).expect("parse tcp");
        assert_eq!(parsed.control, TcpControl::Syn);
        assert_eq!(parsed.dst_port, 443);

        let udp = udp_frame(53000, 443, TEST_GUEST_IP, TEST_PUBLIC_IP, b"quic?");
        assert_eq!(&udp[0..6], TEST_GATEWAY_MAC.as_bytes());
        assert_eq!(&udp[6..12], TEST_GUEST_MAC.as_bytes());
        assert_eq!(u16::from_be_bytes([udp[12], udp[13]]), 0x0800);
        assert_eq!(udp[23], 17);
        assert_eq!(u16::from_be_bytes([udp[34], udp[35]]), 53000);
        assert_eq!(u16::from_be_bytes([udp[36], udp[37]]), 443);
    }
}
