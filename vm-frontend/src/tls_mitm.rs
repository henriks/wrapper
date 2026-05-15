use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, Issuer, KeyPair,
};
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};

#[derive(Debug)]
pub struct TlsMitmAuthority {
    issuer: Issuer<'static, KeyPair>,
    ca_cert: CertificateDer<'static>,
}

#[derive(Debug)]
pub struct TlsMitmCertResolver {
    authority: Arc<TlsMitmAuthority>,
    cache: Mutex<std::collections::HashMap<String, Arc<CertifiedKey>>>,
}

#[derive(Debug)]
pub struct GeneratedServerCertificate {
    pub cert_chain: Vec<CertificateDer<'static>>,
    pub private_key: PrivateKeyDer<'static>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TlsMitmError {
    Io(String),
    InvalidCa(String),
    InvalidHost(String),
    CertificateGeneration(String),
    NativeRoots(String),
    UnsupportedSigningKey(String),
    Tls(String),
    PlaintextWrite(String),
}

#[derive(Debug)]
pub struct GuestTlsSession {
    server: ServerConnection,
}

#[derive(Debug)]
pub struct TlsUpstreamSession {
    client: ClientConnection,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct GuestTlsRead {
    pub plaintext: Vec<u8>,
    pub tls_to_guest: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TlsUpstreamRead {
    pub plaintext: Vec<u8>,
    pub tls_to_upstream: Vec<u8>,
}

impl TlsMitmAuthority {
    pub fn from_files(
        ca_cert_path: impl AsRef<Path>,
        ca_key_path: impl AsRef<Path>,
    ) -> Result<Self, TlsMitmError> {
        let ca_cert = fs::read_to_string(ca_cert_path.as_ref())
            .map_err(|error| TlsMitmError::Io(error.to_string()))?;
        let ca_key = fs::read_to_string(ca_key_path.as_ref())
            .map_err(|error| TlsMitmError::Io(error.to_string()))?;
        Self::from_pem(&ca_cert, &ca_key)
    }

    pub fn from_pem(ca_cert_pem: &str, ca_key_pem: &str) -> Result<Self, TlsMitmError> {
        let ca_key = KeyPair::from_pem(ca_key_pem)
            .map_err(|error| TlsMitmError::InvalidCa(error.to_string()))?;
        let issuer = Issuer::from_ca_cert_pem(ca_cert_pem, ca_key)
            .map_err(|error| TlsMitmError::InvalidCa(error.to_string()))?;
        let ca_cert = rustls_pemfile::certs(&mut ca_cert_pem.as_bytes())
            .next()
            .transpose()
            .map_err(|error| TlsMitmError::InvalidCa(error.to_string()))?
            .ok_or_else(|| TlsMitmError::InvalidCa("missing CA certificate".to_string()))?;
        Ok(Self { issuer, ca_cert })
    }

    pub fn generate_server_certificate(
        &self,
        host: &str,
    ) -> Result<GeneratedServerCertificate, TlsMitmError> {
        let leaf_key = KeyPair::generate()
            .map_err(|error| TlsMitmError::CertificateGeneration(error.to_string()))?;
        let mut params = CertificateParams::new(vec![host.to_string()])
            .map_err(|error| TlsMitmError::InvalidHost(error.to_string()))?;
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, host);
        params
            .extended_key_usages
            .push(ExtendedKeyUsagePurpose::ServerAuth);
        params.use_authority_key_identifier_extension = true;
        let cert = params
            .signed_by(&leaf_key, &self.issuer)
            .map_err(|error| TlsMitmError::CertificateGeneration(error.to_string()))?;

        Ok(GeneratedServerCertificate {
            cert_chain: vec![cert.der().clone(), self.ca_cert.clone()],
            private_key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
        })
    }

    pub fn rustls_certified_key_for_host(&self, host: &str) -> Result<CertifiedKey, TlsMitmError> {
        let generated = self.generate_server_certificate(host)?;
        let signing_key = any_supported_type(&generated.private_key)
            .map_err(|error| TlsMitmError::UnsupportedSigningKey(error.to_string()))?;
        Ok(CertifiedKey::new(generated.cert_chain, signing_key))
    }

    pub fn ca_cert(&self) -> CertificateDer<'static> {
        self.ca_cert.clone()
    }

    pub fn rustls_server_config(self: Arc<Self>) -> Result<ServerConfig, TlsMitmError> {
        Ok(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::DEFAULT_VERSIONS)
                .map_err(|error| TlsMitmError::CertificateGeneration(error.to_string()))?
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(TlsMitmCertResolver::new(self))),
        )
    }
}

impl TlsMitmCertResolver {
    pub fn new(authority: Arc<TlsMitmAuthority>) -> Self {
        Self {
            authority,
            cache: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn cached_host_count(&self) -> usize {
        self.cache.lock().map(|cache| cache.len()).unwrap_or(0)
    }
}

impl GuestTlsSession {
    pub fn new(server_config: Arc<ServerConfig>) -> Result<Self, TlsMitmError> {
        let server = ServerConnection::new(server_config)
            .map_err(|error| TlsMitmError::Tls(error.to_string()))?;
        Ok(Self { server })
    }

    pub fn read_guest_tls(&mut self, bytes: &[u8]) -> Result<GuestTlsRead, TlsMitmError> {
        let mut plaintext = Vec::new();
        let mut tls_to_guest = Vec::new();
        for chunk in bytes.chunks(TLS_FEED_CHUNK_SIZE) {
            let mut reader = chunk;
            self.server
                .read_tls(&mut reader)
                .map_err(|error| TlsMitmError::Io(error.to_string()))?;
            let (chunk_plaintext, chunk_tls_to_guest) =
                process_server_packets_until_idle(&mut self.server)?;
            plaintext.extend_from_slice(&chunk_plaintext);
            tls_to_guest.extend_from_slice(&chunk_tls_to_guest);
        }
        Ok(GuestTlsRead {
            plaintext,
            tls_to_guest,
        })
    }

    pub fn write_guest_plaintext(&mut self, bytes: &[u8]) -> Result<Vec<u8>, TlsMitmError> {
        write_server_plaintext_streaming(&mut self.server, bytes)
    }

    pub fn is_handshaking(&self) -> bool {
        self.server.is_handshaking()
    }

    pub fn server_name(&self) -> Option<&str> {
        self.server.server_name()
    }
}

fn read_plaintext(server: &mut ServerConnection) -> Result<Vec<u8>, TlsMitmError> {
    let mut plaintext = Vec::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        match server.reader().read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => plaintext.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(TlsMitmError::Io(error.to_string())),
        }
    }
    Ok(plaintext)
}

impl TlsUpstreamSession {
    pub fn new(client_config: Arc<ClientConfig>, server_name: &str) -> Result<Self, TlsMitmError> {
        let server_name = ServerName::try_from(server_name.to_string())
            .map_err(|error| TlsMitmError::InvalidHost(error.to_string()))?;
        let client = ClientConnection::new(client_config, server_name)
            .map_err(|error| TlsMitmError::Tls(error.to_string()))?;
        Ok(Self { client })
    }

    pub fn read_upstream_tls(&mut self, bytes: &[u8]) -> Result<TlsUpstreamRead, TlsMitmError> {
        let mut plaintext = Vec::new();
        let mut tls_to_upstream = Vec::new();
        for chunk in bytes.chunks(TLS_FEED_CHUNK_SIZE) {
            let mut reader = chunk;
            self.client
                .read_tls(&mut reader)
                .map_err(|error| TlsMitmError::Io(error.to_string()))?;
            let (chunk_plaintext, chunk_tls_to_upstream) =
                process_client_packets_until_idle(&mut self.client)?;
            plaintext.extend_from_slice(&chunk_plaintext);
            tls_to_upstream.extend_from_slice(&chunk_tls_to_upstream);
        }
        Ok(TlsUpstreamRead {
            plaintext,
            tls_to_upstream,
        })
    }

    pub fn write_upstream_plaintext(&mut self, bytes: &[u8]) -> Result<Vec<u8>, TlsMitmError> {
        write_client_plaintext_streaming(&mut self.client, bytes)
    }

    pub fn drain_tls_to_upstream(&mut self) -> Result<Vec<u8>, TlsMitmError> {
        drain_client_tls_writes(&mut self.client)
    }

    pub fn is_handshaking(&self) -> bool {
        self.client.is_handshaking()
    }

    pub fn wants_write(&self) -> bool {
        self.client.wants_write()
    }
}

const TLS_FEED_CHUNK_SIZE: usize = 1024;

fn process_server_packets_until_idle(
    server: &mut ServerConnection,
) -> Result<(Vec<u8>, Vec<u8>), TlsMitmError> {
    let mut plaintext = Vec::new();
    let mut tls_to_peer = Vec::new();
    for _ in 0..32 {
        let state = server
            .process_new_packets()
            .map_err(|error| TlsMitmError::Tls(error.to_string()))?;
        let new_plaintext = read_plaintext(server)?;
        let new_tls = drain_tls_writes(server)?;
        let made_progress = !new_plaintext.is_empty() || !new_tls.is_empty();
        plaintext.extend_from_slice(&new_plaintext);
        tls_to_peer.extend_from_slice(&new_tls);
        if !made_progress && state.tls_bytes_to_write() == 0 && state.plaintext_bytes_to_read() == 0
        {
            break;
        }
    }
    Ok((plaintext, tls_to_peer))
}

fn process_client_packets_until_idle(
    client: &mut ClientConnection,
) -> Result<(Vec<u8>, Vec<u8>), TlsMitmError> {
    let mut plaintext = Vec::new();
    let mut tls_to_peer = Vec::new();
    for _ in 0..32 {
        let state = client
            .process_new_packets()
            .map_err(|error| TlsMitmError::Tls(error.to_string()))?;
        let new_plaintext = read_client_plaintext(client)?;
        let new_tls = drain_client_tls_writes(client)?;
        let made_progress = !new_plaintext.is_empty() || !new_tls.is_empty();
        plaintext.extend_from_slice(&new_plaintext);
        tls_to_peer.extend_from_slice(&new_tls);
        if !made_progress && state.tls_bytes_to_write() == 0 && state.plaintext_bytes_to_read() == 0
        {
            break;
        }
    }
    Ok((plaintext, tls_to_peer))
}

pub fn rustls_client_config_with_native_roots() -> Result<ClientConfig, TlsMitmError> {
    rustls_client_config_with_roots(native_root_store()?)
}

pub fn rustls_client_config_with_roots(roots: RootCertStore) -> Result<ClientConfig, TlsMitmError> {
    Ok(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(rustls::DEFAULT_VERSIONS)
            .map_err(|error| TlsMitmError::CertificateGeneration(error.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

pub fn native_root_store() -> Result<RootCertStore, TlsMitmError> {
    let loaded = rustls_native_certs::load_native_certs();
    if loaded.certs.is_empty() {
        let error = loaded
            .errors
            .first()
            .map(|error| format!("{error:?}"))
            .unwrap_or_else(|| "no native root certificates loaded".to_string());
        return Err(TlsMitmError::NativeRoots(error));
    }
    let mut roots = RootCertStore::empty();
    for cert in loaded.certs {
        roots
            .add(cert)
            .map_err(|error| TlsMitmError::NativeRoots(error.to_string()))?;
    }
    Ok(roots)
}

fn read_client_plaintext(client: &mut ClientConnection) -> Result<Vec<u8>, TlsMitmError> {
    let mut plaintext = Vec::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        match client.reader().read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => plaintext.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(TlsMitmError::Io(error.to_string())),
        }
    }
    Ok(plaintext)
}

fn drain_client_tls_writes(client: &mut ClientConnection) -> Result<Vec<u8>, TlsMitmError> {
    let mut output = Vec::new();
    while client.wants_write() {
        let before = output.len();
        client
            .write_tls(&mut output)
            .map_err(|error| TlsMitmError::Io(error.to_string()))?;
        if output.len() == before {
            break;
        }
    }
    Ok(output)
}

fn drain_tls_writes(server: &mut ServerConnection) -> Result<Vec<u8>, TlsMitmError> {
    let mut output = Vec::new();
    while server.wants_write() {
        let before = output.len();
        server
            .write_tls(&mut output)
            .map_err(|error| TlsMitmError::Io(error.to_string()))?;
        if output.len() == before {
            break;
        }
    }
    Ok(output)
}

fn write_server_plaintext_streaming(
    server: &mut ServerConnection,
    bytes: &[u8],
) -> Result<Vec<u8>, TlsMitmError> {
    let mut output = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        match server.writer().write(&bytes[offset..]) {
            Ok(0) => {
                let drained = drain_tls_writes(server)?;
                if drained.is_empty() {
                    return Err(TlsMitmError::PlaintextWrite(
                        "rustls server writer made no progress".to_string(),
                    ));
                }
                output.extend_from_slice(&drained);
            }
            Ok(count) => {
                offset += count;
                output.extend_from_slice(&drain_tls_writes(server)?);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                output.extend_from_slice(&drain_tls_writes(server)?);
            }
            Err(error) => return Err(TlsMitmError::PlaintextWrite(error.to_string())),
        }
    }
    output.extend_from_slice(&drain_tls_writes(server)?);
    Ok(output)
}

fn write_client_plaintext_streaming(
    client: &mut ClientConnection,
    bytes: &[u8],
) -> Result<Vec<u8>, TlsMitmError> {
    let mut output = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        match client.writer().write(&bytes[offset..]) {
            Ok(0) => {
                let drained = drain_client_tls_writes(client)?;
                if drained.is_empty() {
                    return Err(TlsMitmError::PlaintextWrite(
                        "rustls client writer made no progress".to_string(),
                    ));
                }
                output.extend_from_slice(&drained);
            }
            Ok(count) => {
                offset += count;
                output.extend_from_slice(&drain_client_tls_writes(client)?);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                output.extend_from_slice(&drain_client_tls_writes(client)?);
            }
            Err(error) => return Err(TlsMitmError::PlaintextWrite(error.to_string())),
        }
    }
    output.extend_from_slice(&drain_client_tls_writes(client)?);
    Ok(output)
}

impl ResolvesServerCert for TlsMitmCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let host = client_hello.server_name()?.to_string();
        let mut cache = self.cache.lock().ok()?;
        if let Some(certified_key) = cache.get(&host) {
            return Some(certified_key.clone());
        }
        let certified_key = Arc::new(self.authority.rustls_certified_key_for_host(&host).ok()?);
        cache.insert(host, certified_key.clone());
        Some(certified_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, IsCa, KeyUsagePurpose};
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConnection};
    use std::io::{Read, Write};

    #[test]
    fn loads_ca_and_generates_per_host_certificate_chain() {
        let (ca_cert, ca_key) = test_ca_pem();
        let authority = TlsMitmAuthority::from_pem(&ca_cert, &ca_key).expect("authority");

        let generated = authority
            .generate_server_certificate("example.com")
            .expect("leaf");

        assert_eq!(generated.cert_chain.len(), 2);
        assert!(!generated.cert_chain[0].as_ref().is_empty());
        assert!(!generated.cert_chain[1].as_ref().is_empty());
        assert!(matches!(generated.private_key, PrivateKeyDer::Pkcs8(_)));
    }

    #[test]
    fn rejects_invalid_ca_material() {
        let error = TlsMitmAuthority::from_pem("not a cert", "not a key").expect_err("invalid ca");

        assert!(matches!(error, TlsMitmError::InvalidCa(_)));
    }

    #[test]
    fn builds_rustls_certified_key_for_host() {
        let (ca_cert, ca_key) = test_ca_pem();
        let authority = TlsMitmAuthority::from_pem(&ca_cert, &ca_key).expect("authority");

        let key = authority
            .rustls_certified_key_for_host("api.example.com")
            .expect("certified key");

        assert_eq!(key.cert.len(), 2);
    }

    #[test]
    fn rustls_server_config_accepts_client_that_trusts_ca() {
        let (ca_cert, ca_key) = test_ca_pem();
        let authority = Arc::new(TlsMitmAuthority::from_pem(&ca_cert, &ca_key).expect("authority"));
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let client_config = Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::DEFAULT_VERSIONS)
                .expect("versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let server_config = Arc::new(authority.rustls_server_config().expect("server config"));
        let mut client = ClientConnection::new(
            client_config,
            ServerName::try_from("example.com")
                .expect("server name")
                .to_owned(),
        )
        .expect("client");
        let mut server = ServerConnection::new(server_config).expect("server");

        pump_tls_pair(&mut client, &mut server);
        assert!(!client.is_handshaking());
        assert!(!server.is_handshaking());

        client
            .writer()
            .write_all(b"GET /secret HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .expect("write plaintext");
        pump_tls_pair(&mut client, &mut server);

        let mut plaintext = [0; 1024];
        let count = server
            .reader()
            .read(&mut plaintext)
            .expect("read plaintext");
        assert_eq!(
            &plaintext[..count],
            b"GET /secret HTTP/1.1\r\nHost: example.com\r\n\r\n"
        );
    }

    #[test]
    fn guest_tls_session_decrypts_request_and_encrypts_response() {
        let (ca_cert, ca_key) = test_ca_pem();
        let authority = Arc::new(TlsMitmAuthority::from_pem(&ca_cert, &ca_key).expect("authority"));
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let client_config = Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::DEFAULT_VERSIONS)
                .expect("versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let mut client = ClientConnection::new(
            client_config,
            ServerName::try_from("example.com")
                .expect("server name")
                .to_owned(),
        )
        .expect("client");
        let mut session =
            GuestTlsSession::new(Arc::new(authority.rustls_server_config().expect("server")))
                .expect("session");

        let client_hello = drain_client_tls(&mut client);
        let server_hello = session
            .read_guest_tls(&client_hello)
            .expect("server hello")
            .tls_to_guest;
        feed_client_tls(&mut client, &server_hello);
        let client_finished = drain_client_tls(&mut client);
        let read = session
            .read_guest_tls(&client_finished)
            .expect("client finished");
        if !read.tls_to_guest.is_empty() {
            feed_client_tls(&mut client, &read.tls_to_guest);
        }
        assert!(!client.is_handshaking());
        assert!(!session.is_handshaking());

        client
            .writer()
            .write_all(b"GET /tls HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .expect("request");
        let request_tls = drain_client_tls(&mut client);
        let read = session.read_guest_tls(&request_tls).expect("request read");
        assert_eq!(
            read.plaintext,
            b"GET /tls HTTP/1.1\r\nHost: example.com\r\n\r\n"
        );

        let response_tls = session
            .write_guest_plaintext(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
            .expect("response");
        feed_client_tls(&mut client, &response_tls);
        let response = read_client_plaintext(&mut client);
        assert_eq!(response, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
    }

    #[test]
    fn guest_tls_session_handles_large_fragmented_response_without_bad_record_mac() {
        let (ca_cert, ca_key) = test_ca_pem();
        let authority = Arc::new(TlsMitmAuthority::from_pem(&ca_cert, &ca_key).expect("authority"));
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let client_config = Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::DEFAULT_VERSIONS)
                .expect("versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let mut client = ClientConnection::new(
            client_config,
            ServerName::try_from("registry.npmjs.org")
                .expect("server name")
                .to_owned(),
        )
        .expect("client");
        let mut session =
            GuestTlsSession::new(Arc::new(authority.rustls_server_config().expect("server")))
                .expect("session");
        complete_guest_tls_handshake(&mut client, &mut session);

        let mut body = vec![b'J'; 192 * 1024];
        body[0..8].copy_from_slice(b"npm-meta");
        let mut response =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        response.extend_from_slice(&body);
        let response_tls = session
            .write_guest_plaintext(&response)
            .expect("large response tls");

        let mut decrypted = Vec::new();
        for chunk in response_tls.chunks(137) {
            feed_client_tls(&mut client, chunk);
            decrypted.extend(read_client_plaintext_unbounded(&mut client));
        }
        decrypted.extend(read_client_plaintext_unbounded(&mut client));

        assert_eq!(decrypted.len(), response.len());
        assert_eq!(&decrypted[..15], b"HTTP/1.1 200 OK");
        assert!(decrypted.windows(8).any(|window| window == b"npm-meta"));
    }

    #[test]
    fn upstream_tls_session_encrypts_request_and_decrypts_response() {
        let (ca_cert, ca_key) = test_ca_pem();
        let authority = TlsMitmAuthority::from_pem(&ca_cert, &ca_key).expect("authority");
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let client_config = Arc::new(rustls_client_config_with_roots(roots).expect("client"));
        let generated = authority
            .generate_server_certificate("upstream.example")
            .expect("server cert");
        let server_config = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::DEFAULT_VERSIONS)
                .expect("versions")
                .with_no_client_auth()
                .with_single_cert(generated.cert_chain, generated.private_key)
                .expect("server config"),
        );
        let mut upstream =
            TlsUpstreamSession::new(client_config, "upstream.example").expect("upstream");
        let mut server = ServerConnection::new(server_config).expect("server");

        pump_upstream_pair(&mut upstream, &mut server);
        assert!(!upstream.is_handshaking());
        assert!(!server.is_handshaking());

        let request_tls = upstream
            .write_upstream_plaintext(b"GET / HTTP/1.1\r\nHost: upstream.example\r\n\r\n")
            .expect("request");
        feed_server_tls(&mut server, &request_tls);
        let mut request = [0; 1024];
        let count = server.reader().read(&mut request).expect("server request");
        assert_eq!(
            &request[..count],
            b"GET / HTTP/1.1\r\nHost: upstream.example\r\n\r\n"
        );

        server
            .writer()
            .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
            .expect("response");
        let response_tls = drain_server_tls(&mut server);
        let response = upstream
            .read_upstream_tls(&response_tls)
            .expect("response read");
        assert_eq!(response.plaintext, b"HTTP/1.1 204 No Content\r\n\r\n");
    }

    #[test]
    fn upstream_tls_session_emits_request_after_client_finished() {
        let (ca_cert, ca_key) = test_ca_pem();
        let authority = TlsMitmAuthority::from_pem(&ca_cert, &ca_key).expect("authority");
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let client_config = Arc::new(rustls_client_config_with_roots(roots).expect("client"));
        let generated = authority
            .generate_server_certificate("upstream.example")
            .expect("server cert");
        let server_config = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::DEFAULT_VERSIONS)
                .expect("versions")
                .with_no_client_auth()
                .with_single_cert(generated.cert_chain, generated.private_key)
                .expect("server config"),
        );
        let mut upstream =
            TlsUpstreamSession::new(client_config, "upstream.example").expect("upstream");
        let mut server = ServerConnection::new(server_config).expect("server");

        let client_hello = upstream.drain_tls_to_upstream().expect("client hello");
        assert!(!client_hello.is_empty());
        feed_server_tls(&mut server, &client_hello);

        let server_flight = drain_server_tls(&mut server);
        assert!(!server_flight.is_empty());
        let read = upstream
            .read_upstream_tls(&server_flight)
            .expect("server flight");
        assert!(read.plaintext.is_empty());
        assert!(!read.tls_to_upstream.is_empty());
        feed_server_tls(&mut server, &read.tls_to_upstream);

        let request_tls = upstream
            .write_upstream_plaintext(b"GET / HTTP/1.1\r\nHost: upstream.example\r\n\r\n")
            .expect("request tls");
        assert!(
            !request_tls.is_empty(),
            "rustls accepted plaintext after client Finished but emitted no TLS"
        );
    }

    fn test_ca_pem() -> (String, String) {
        let ca_key = KeyPair::generate().expect("ca key");
        let mut params =
            CertificateParams::new(vec!["agentvm-test-ca".to_string()]).expect("ca params");
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
        let ca_cert = params.self_signed(&ca_key).expect("ca cert");
        (ca_cert.pem(), ca_key.serialize_pem())
    }

    fn pump_tls_pair(client: &mut ClientConnection, server: &mut ServerConnection) {
        for _ in 0..128 {
            let progressed =
                pump_client_to_server(client, server) | pump_server_to_client(server, client);
            if !progressed && !client.wants_write() && !server.wants_write() {
                break;
            }
        }
    }

    fn pump_client_to_server(client: &mut ClientConnection, server: &mut ServerConnection) -> bool {
        let mut bytes = Vec::new();
        let wrote = client.write_tls(&mut bytes).expect("write client tls");
        if bytes.is_empty() {
            return wrote > 0;
        }
        server
            .read_tls(&mut bytes.as_slice())
            .expect("server read tls");
        server.process_new_packets().expect("server process tls");
        true
    }

    fn pump_server_to_client(server: &mut ServerConnection, client: &mut ClientConnection) -> bool {
        let mut bytes = Vec::new();
        let wrote = server.write_tls(&mut bytes).expect("write server tls");
        if bytes.is_empty() {
            return wrote > 0;
        }
        client
            .read_tls(&mut bytes.as_slice())
            .expect("client read tls");
        client.process_new_packets().expect("client process tls");
        true
    }

    fn drain_client_tls(client: &mut ClientConnection) -> Vec<u8> {
        let mut bytes = Vec::new();
        client.write_tls(&mut bytes).expect("client write tls");
        bytes
    }

    fn feed_client_tls(client: &mut ClientConnection, bytes: &[u8]) {
        let mut reader = bytes;
        client.read_tls(&mut reader).expect("client read tls");
        client.process_new_packets().expect("client process tls");
    }

    fn complete_guest_tls_handshake(client: &mut ClientConnection, session: &mut GuestTlsSession) {
        let client_hello = drain_client_tls(client);
        let server_hello = session
            .read_guest_tls(&client_hello)
            .expect("server hello")
            .tls_to_guest;
        feed_client_tls(client, &server_hello);
        let client_finished = drain_client_tls(client);
        let read = session
            .read_guest_tls(&client_finished)
            .expect("client finished");
        if !read.tls_to_guest.is_empty() {
            feed_client_tls(client, &read.tls_to_guest);
        }
        assert!(!client.is_handshaking());
        assert!(!session.is_handshaking());
    }

    fn read_client_plaintext(client: &mut ClientConnection) -> Vec<u8> {
        let mut plaintext = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            match client.reader().read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => plaintext.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("client plaintext read failed: {error}"),
            }
        }
        plaintext
    }

    fn read_client_plaintext_unbounded(client: &mut ClientConnection) -> Vec<u8> {
        let mut plaintext = Vec::new();
        let mut buffer = [0; 16 * 1024];
        loop {
            match client.reader().read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => plaintext.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("client plaintext read failed: {error}"),
            }
        }
        plaintext
    }

    fn pump_upstream_pair(upstream: &mut TlsUpstreamSession, server: &mut ServerConnection) {
        for _ in 0..128 {
            let mut progressed = false;
            let client_tls = upstream.drain_tls_to_upstream().expect("client tls");
            if !client_tls.is_empty() {
                feed_server_tls(server, &client_tls);
                progressed = true;
            }
            let server_tls = drain_server_tls(server);
            if !server_tls.is_empty() {
                let read = upstream
                    .read_upstream_tls(&server_tls)
                    .expect("upstream read");
                assert!(read.plaintext.is_empty());
                if !read.tls_to_upstream.is_empty() {
                    feed_server_tls(server, &read.tls_to_upstream);
                }
                progressed = true;
            }
            if !progressed && !upstream.is_handshaking() && !server.is_handshaking() {
                break;
            }
        }
    }

    fn feed_server_tls(server: &mut ServerConnection, bytes: &[u8]) {
        let mut reader = bytes;
        server.read_tls(&mut reader).expect("server read tls");
        server.process_new_packets().expect("server process tls");
    }

    fn drain_server_tls(server: &mut ServerConnection) -> Vec<u8> {
        let mut bytes = Vec::new();
        server.write_tls(&mut bytes).expect("server write tls");
        bytes
    }
}
