use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
};

pub(crate) type TlsBootstrapResult<T> = Result<T, TlsBootstrapError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum TlsBootstrapError {
    #[error("failed to create wrapper MITM CA directory {path}: {source}")]
    CreateCaDirectory {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to generate MITM CA key: {source}")]
    GenerateCaKey { source: rcgen::Error },
    #[error("failed to create MITM CA params: {source}")]
    CreateCaParams { source: rcgen::Error },
    #[error("failed to generate MITM CA certificate: {source}")]
    GenerateCaCertificate { source: rcgen::Error },
    #[error("failed to write wrapper MITM CA certificate {path}: {source}")]
    WriteCaCertificate {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to write private key {path}: {source}")]
    WritePrivateKey {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to restrict private key permissions {path}: {source}")]
    RestrictPrivateKeyPermissions {
        path: String,
        source: std::io::Error,
    },
}

impl From<TlsBootstrapError> for String {
    fn from(error: TlsBootstrapError) -> Self {
        error.to_string()
    }
}

#[derive(Debug)]
pub(crate) struct MitmCaPaths {
    pub(crate) cert: PathBuf,
    pub(crate) key: PathBuf,
}

pub(crate) fn ensure_wrapper_mitm_ca(project: &Path) -> TlsBootstrapResult<MitmCaPaths> {
    let dir = project.join(".sandbox/docker-vm/ca");
    let cert = dir.join("mitm-ca.crt");
    let key = dir.join("mitm-ca.key");
    if cert.exists() && key.exists() {
        return Ok(MitmCaPaths { cert, key });
    }

    fs::create_dir_all(&dir).map_err(|source| TlsBootstrapError::CreateCaDirectory {
        path: dir.display().to_string(),
        source,
    })?;

    let key_pair =
        KeyPair::generate().map_err(|source| TlsBootstrapError::GenerateCaKey { source })?;
    let mut params = CertificateParams::new(Vec::<String>::new())
        .map_err(|source| TlsBootstrapError::CreateCaParams { source })?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "agentvm project MITM CA");
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params.key_usages.push(KeyUsagePurpose::KeyCertSign);
    params.key_usages.push(KeyUsagePurpose::CrlSign);

    let ca_cert = params
        .self_signed(&key_pair)
        .map_err(|source| TlsBootstrapError::GenerateCaCertificate { source })?;
    fs::write(&cert, ca_cert.pem()).map_err(|source| TlsBootstrapError::WriteCaCertificate {
        path: cert.display().to_string(),
        source,
    })?;
    write_private_key(&key, &key_pair.serialize_pem())?;
    Ok(MitmCaPaths { cert, key })
}

fn write_private_key(path: &Path, pem: &str) -> TlsBootstrapResult<()> {
    #[cfg(unix)]
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|source| TlsBootstrapError::WritePrivateKey {
                path: path.display().to_string(),
                source,
            })?;
        file.write_all(pem.as_bytes())
            .map_err(|source| TlsBootstrapError::WritePrivateKey {
                path: path.display().to_string(),
                source,
            })?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
            TlsBootstrapError::RestrictPrivateKeyPermissions {
                path: path.display().to_string(),
                source,
            }
        })?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, pem).map_err(|source| TlsBootstrapError::WritePrivateKey {
            path: path.display().to_string(),
            source,
        })?;
    }
    Ok(())
}
