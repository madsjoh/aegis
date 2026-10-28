//! Certificate authority and leaf minting.
//!
//! Owns the certificate authority the gateway uses to mint leaf certificates
//! for TLS interception. The CA key pair is persisted as `ca.key` and `ca.crt`
//! in the configured directory, and per-hostname leaves are cached in memory.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;

const CA_KEY_FILE: &str = "ca.key";
const CA_CERT_FILE: &str = "ca.crt";
const CA_COMMON_NAME: &str = "Aegis Gateway CA";

/// A certificate authority that mints per-hostname leaf certificates.
pub struct Authority {
    key: KeyPair,
    cert: Certificate,
    cert_pem: Vec<u8>,
    leaves: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

/// An error that can occur while operating the certificate authority.
#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error("failed to create CA directory: {0}")]
    CreateDirectory(#[source] std::io::Error),
    #[error("failed to read CA private key: {0}")]
    ReadKey(#[source] std::io::Error),
    #[error("failed to read CA certificate: {0}")]
    ReadCertificate(#[source] std::io::Error),
    #[error("failed to parse CA private key: {0}")]
    ParseKey(#[source] rcgen::Error),
    #[error("failed to generate CA key pair: {0}")]
    GenerateKey(#[source] rcgen::Error),
    #[error("failed to generate CA certificate: {0}")]
    GenerateCertificate(#[source] rcgen::Error),
    #[error("failed to write CA private key: {0}")]
    WriteKey(#[source] std::io::Error),
    #[error("failed to write CA certificate: {0}")]
    WriteCertificate(#[source] std::io::Error),
    #[error("failed to mint leaf certificate: {0}")]
    MintLeaf(#[source] rcgen::Error),
    #[error("failed to build leaf signing key: {0}")]
    BuildSigningKey(#[source] rustls::Error),
}

impl Authority {
    /// Load the CA key pair from `dir`, generating and persisting one first
    /// when the files are absent.
    pub fn load_or_create(dir: &Path) -> Result<Authority, CaError> {
        let key_path = dir.join(CA_KEY_FILE);
        let cert_path = dir.join(CA_CERT_FILE);

        let (key, cert, cert_pem) = if key_path.exists() && cert_path.exists() {
            let key_pem = fs::read_to_string(&key_path).map_err(CaError::ReadKey)?;
            let cert_pem = fs::read_to_string(&cert_path).map_err(CaError::ReadCertificate)?;
            let key = KeyPair::from_pem(&key_pem).map_err(CaError::ParseKey)?;
            let cert = ca_certificate(&key)?;
            (key, cert, cert_pem.into_bytes())
        } else {
            fs::create_dir_all(dir).map_err(CaError::CreateDirectory)?;
            let key = KeyPair::generate().map_err(CaError::GenerateKey)?;
            let cert = ca_certificate(&key)?;
            let key_pem = key.serialize_pem();
            let cert_pem = cert.pem();
            write_private_key(&key_path, key_pem.as_bytes())?;
            fs::write(&cert_path, cert_pem.as_bytes()).map_err(CaError::WriteCertificate)?;
            (key, cert, cert_pem.into_bytes())
        };

        Ok(Authority {
            key,
            cert,
            cert_pem,
            leaves: Mutex::new(HashMap::new()),
        })
    }

    /// Return the CA certificate in PEM format.
    pub fn ca_pem(&self) -> Vec<u8> {
        self.cert_pem.clone()
    }

    /// Mint a leaf certificate for `hostname`, signed by the CA, and return it
    /// as a rustls signing key. Leaves are cached by hostname.
    pub fn leaf_certified_key(&self, hostname: &str) -> Result<Arc<CertifiedKey>, CaError> {
        if let Some(cached) = self.leaves.lock().unwrap().get(hostname) {
            return Ok(Arc::clone(cached));
        }

        let leaf_key = KeyPair::generate().map_err(CaError::MintLeaf)?;
        let mut params =
            CertificateParams::new(vec![hostname.to_string()]).map_err(CaError::MintLeaf)?;
        params.is_ca = IsCa::NoCa;
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, hostname);
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];

        let leaf_cert = params
            .signed_by(&leaf_key, &self.cert, &self.key)
            .map_err(CaError::MintLeaf)?;

        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(CaError::BuildSigningKey)?;

        let certified = Arc::new(CertifiedKey::new(
            vec![leaf_cert.der().clone()],
            signing_key,
        ));
        self.leaves
            .lock()
            .unwrap()
            .insert(hostname.to_string(), Arc::clone(&certified));
        Ok(certified)
    }
}

/// Build the self-signed CA certificate for a key pair with fixed parameters.
///
/// The parameters are deterministic so that a CA loaded from disk reconstructs
/// the same subject name, key identifier method, and key usages it was issued
/// with, which keeps leaf chains consistent across restarts.
fn ca_certificate(key: &KeyPair) -> Result<Certificate, CaError> {
    let mut params =
        CertificateParams::new(Vec::<String>::new()).map_err(CaError::GenerateCertificate)?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, CA_COMMON_NAME);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.use_authority_key_identifier_extension = true;
    params
        .self_signed(key)
        .map_err(CaError::GenerateCertificate)
}

/// Write a private key PEM with mode 0600.
fn write_private_key(path: &Path, pem: &[u8]) -> Result<(), CaError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(CaError::WriteKey)?;
    file.write_all(pem).map_err(CaError::WriteKey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use x509_parser::extensions::GeneralName;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-ca-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn create_yields_nonempty_pem_and_0600_key() {
        let dir = temp_dir("create");
        let authority = Authority::load_or_create(&dir).expect("create succeeds");
        assert!(!authority.ca_pem().is_empty());
        let mode = std::fs::metadata(dir.join("ca.key"))
            .expect("key file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_returns_same_ca() {
        let dir = temp_dir("reload");
        let first = Authority::load_or_create(&dir).expect("create succeeds");
        let first_pem = first.ca_pem();
        let second = Authority::load_or_create(&dir).expect("reload succeeds");
        assert_eq!(first_pem, second.ca_pem());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaf_has_hostname_in_san() {
        let dir = temp_dir("leaf");
        let authority = Authority::load_or_create(&dir).expect("create succeeds");
        let certified = authority
            .leaf_certified_key("api.github.com")
            .expect("leaf mints");
        let der = certified.cert[0].as_ref();
        let (_, parsed) = x509_parser::parse_x509_certificate(der).expect("leaf parses");
        let san = parsed
            .subject_alternative_name()
            .expect("SAN readable")
            .expect("SAN present");
        let dns_names: Vec<&str> = san
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::DNSName(name) => Some(*name),
                _ => None,
            })
            .collect();
        assert!(
            dns_names.contains(&"api.github.com"),
            "SAN DNS names: {dns_names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
