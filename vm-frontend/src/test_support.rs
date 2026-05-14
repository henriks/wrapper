#![allow(dead_code)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
};
use smoltcp::time::Instant;

use crate::launch::LaunchError;
use crate::tls_mitm::TlsMitmAuthority;
use crate::vmnet_stream::QemuFrameIo;
use crate::{FrontendConfig, GuestNetwork, RuntimePaths};

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
}
