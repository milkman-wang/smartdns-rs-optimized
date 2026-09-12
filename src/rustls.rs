use std::{
    fs::{self, File},
    io::{self, BufReader},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
pub type Certificate = CertificateDer<'static>;
pub type PrivateKey = PrivateKeyDer<'static>;

pub use rustls::server::ResolvesServerCert;
use rustls::{ClientConfig, ServerConfig, sign::CertifiedKey};

use crate::log::warn;
mod certificate;
pub use certificate::prepare_server_certificate;

#[derive(Clone)]
pub struct TlsClientConfigBundle {
    pub normal: Arc<ClientConfig>,
    pub sni_off: Arc<ClientConfig>,
    pub verify_off: Arc<ClientConfig>,
    verifier: Arc<dyn rustls::client::danger::ServerCertVerifier>,
}

impl TlsClientConfigBundle {
    pub fn new(ca_path: Option<PathBuf>, ca_file: Option<PathBuf>) -> Self {
        let (config, verifier) = Self::create_tls_client_config(
            [ca_path, ca_file]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .as_slice(),
        );

        let sni_off = {
            let mut sni_off = config.clone();
            sni_off.enable_sni = false;
            sni_off
        };

        let verify_off = {
            let mut verify_off = config.clone();
            verify_off
                .dangerous()
                .set_certificate_verifier(Arc::new(NoCertificateVerification));

            verify_off
        };

        Self {
            normal: Arc::new(config),
            sni_off: Arc::new(sni_off),
            verify_off: Arc::new(verify_off),
            verifier,
        }
    }

    fn create_tls_client_config(
        paths: &[PathBuf],
    ) -> (
        ClientConfig,
        Arc<dyn rustls::client::danger::ServerCertVerifier>,
    ) {
        use rustls::RootCertStore;

        let mut root_store = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };

        let certs = {
            let certs1 = rustls_native_certs::load_native_certs().certs;

            let certs2 = paths
                .iter()
                .filter_map(|path| match load_certs_from_path(path.as_path()) {
                    Ok(certs) => Some(certs),
                    Err(err) => {
                        warn!("load certs from path failed.{}", err);
                        None
                    }
                })
                .flatten();

            certs1.into_iter().chain(certs2)
        };

        for cert in certs {
            root_store.add(cert).unwrap_or_else(|err| {
                warn!("load certs from path failed.{}", err);
            })
        }

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(root_store),
            provider.clone(),
        )
        .build()
        .unwrap();
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone())
            .with_no_client_auth();
        (config, verifier)
    }

    pub fn for_upstream(
        &self,
        verify: bool,
        sni: bool,
        pin: Option<&str>,
    ) -> anyhow::Result<Arc<ClientConfig>> {
        let mut config = if verify {
            self.normal.as_ref()
        } else {
            self.verify_off.as_ref()
        }
        .clone();
        config.enable_sni = sni;
        if let Some(pin) = pin {
            config
                .dangerous()
                .set_certificate_verifier(Arc::new(PinnedCertificateVerifier {
                    pin: parse_spki_pin(pin)?,
                    verifier: if verify {
                        self.verifier.clone()
                    } else {
                        Arc::new(NoCertificateVerification)
                    },
                }));
        }
        Ok(Arc::new(config))
    }
}

pub fn parse_spki_pin(pin: &str) -> anyhow::Result<[u8; 32]> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(pin)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("SPKI pin must encode a 32-byte SHA-256 digest"))
}

pub fn load_private_key(
    path: &Path,
    password: Option<&str>,
) -> anyhow::Result<PrivateKeyDer<'static>> {
    let bytes = fs::read(path)?;
    if let Ok(key) = PrivateKeyDer::from_pem_slice(&bytes) {
        return Ok(key);
    }
    if let Ok(key) = PrivateKeyDer::try_from(bytes.as_slice()) {
        return Ok(key.clone_key());
    }
    let password = password.ok_or_else(|| {
        anyhow::anyhow!(
            "private key is encrypted or unsupported; set bind-cert-key-pass for encrypted PKCS#8"
        )
    })?;
    let document;
    let der = if bytes.starts_with(b"-----BEGIN") {
        let (label, decoded) = pkcs8::SecretDocument::from_pem(std::str::from_utf8(&bytes)?)?;
        anyhow::ensure!(
            label == "ENCRYPTED PRIVATE KEY",
            "expected an encrypted PKCS#8 key"
        );
        document = decoded;
        document.as_bytes()
    } else {
        &bytes
    };
    let encrypted = pkcs8::EncryptedPrivateKeyInfo::try_from(der)?;
    let decrypted = encrypted.decrypt(password)?;
    PrivateKeyDer::try_from(decrypted.as_bytes().to_vec()).map_err(|error| anyhow::anyhow!(error))
}

#[derive(Debug)]
struct PinnedCertificateVerifier {
    pin: [u8; 32],
    verifier: Arc<dyn rustls::client::danger::ServerCertVerifier>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedCertificateVerifier {
    fn verify_server_cert(
        &self,
        certificate: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        use sha2::{Digest, Sha256};
        let (_, parsed) =
            x509_parser::parse_x509_certificate(certificate.as_ref()).map_err(|_| {
                rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
            })?;
        let actual: [u8; 32] = Sha256::digest(parsed.public_key().raw).into();
        if actual != self.pin {
            return Err(rustls::Error::General("TLS SPKI pin mismatch".into()));
        }
        self.verifier
            .verify_server_cert(certificate, intermediates, server_name, ocsp, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
pub(super) struct NoCertificateVerification;

impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        use rustls::SignatureScheme::*;
        vec![
            RSA_PKCS1_SHA1,
            ECDSA_SHA1_Legacy,
            RSA_PKCS1_SHA256,
            ECDSA_NISTP256_SHA256,
            RSA_PKCS1_SHA384,
            ECDSA_NISTP384_SHA384,
            RSA_PKCS1_SHA512,
            ECDSA_NISTP521_SHA512,
            RSA_PSS_SHA256,
            RSA_PSS_SHA384,
            RSA_PSS_SHA512,
            ED25519,
            ED448,
        ]
    }
}

/// Load certificates from specific directory or file.
pub fn load_certs_from_path(path: &Path) -> Result<Vec<Certificate>, io::Error> {
    if path.is_dir() {
        let mut certs = vec![];
        for entry in path.read_dir()? {
            let path = entry?.path();
            if path.is_file() {
                certs.extend(load_pem_certs(path.as_path())?);
            }
        }
        Ok(certs)
    } else {
        load_pem_certs(path)
    }
}

fn load_pem_certs(path: &Path) -> Result<Vec<Certificate>, io::Error> {
    let mut file = BufReader::new(File::open(path)?);

    match rustls_pemfile::certs(&mut file).collect() {
        Ok(certs) => Ok(certs),
        Err(err) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Could not load PEM file {err} {path:?}"),
        )),
    }
}

#[cfg(feature = "dns-over-tls")]
pub fn tls_server_config(
    protocol: &[u8],
    server_cert_resolver: Arc<dyn ResolvesServerCert>,
) -> Result<ServerConfig, io::Error> {
    let mut config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| io::Error::other(format!("error creating TLS acceptor: {e}")))?
            .with_no_client_auth()
            .with_cert_resolver(server_cert_resolver);

    config.alpn_protocols = vec![protocol.to_vec()];
    Ok(config)
}

#[derive(Debug)]
pub struct TlsServerCertResolver {
    path: PathBuf,
    private_key: PathBuf,
    certified_key: RwLock<Arc<CertifiedKey>>,
}

impl TlsServerCertResolver {
    pub fn new(
        cert_path: &Path,
        key_path: &Path,
        password: Option<&str>,
    ) -> Result<Self, crate::Error> {
        let certified_key = Self::load(cert_path, key_path, password)?;
        Ok(TlsServerCertResolver {
            path: cert_path.to_path_buf(),
            private_key: key_path.to_path_buf(),
            certified_key: RwLock::new(Arc::new(certified_key)),
        })
    }

    pub fn load(
        cert_path: &Path,
        key_path: &Path,
        password: Option<&str>,
    ) -> Result<CertifiedKey, crate::Error> {
        use crate::Error;
        use rustls::crypto::ring::default_provider;

        let cert_chain = CertificateDer::pem_file_iter(cert_path)
            .map_err(|e| {
                Error::LoadCertificateFailed(
                    cert_path.to_path_buf(),
                    format!(
                        "failed to read cert chain from {}: {e}",
                        cert_path.display()
                    ),
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                Error::LoadCertificateFailed(
                    cert_path.to_path_buf(),
                    format!(
                        "failed to parse cert chain from {}: {e}",
                        cert_path.display()
                    ),
                )
            })?;

        let key = load_private_key(key_path, password).map_err(|error| {
            Error::LoadCertificateKeyFailed(key_path.to_path_buf(), error.to_string())
        })?;

        let certified_key =
            CertifiedKey::from_der(cert_chain, key, &default_provider()).map_err(|err| {
                Error::LoadCertificateKeyFailed(
                    key_path.to_path_buf(),
                    format!("failed to read certificate and keys: {err:?}"),
                )
            })?;

        Ok(certified_key)
    }
}

impl ResolvesServerCert for TlsServerCertResolver {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.certified_key.read().ok().as_deref().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    #[test]
    fn test_encrypted_private_key_password_is_used() {
        let key = rcgen::KeyPair::generate().unwrap();
        let der = key.serialize_der();
        let info = pkcs8::PrivateKeyInfo::try_from(der.as_slice()).unwrap();
        let params =
            pkcs8::pkcs5::pbes2::Parameters::pbkdf2_sha256_aes256cbc(10000, &[1; 16], &[2; 16])
                .unwrap();
        let encrypted = info.encrypt_with_params(params, "test password").unwrap();
        let file =
            std::env::temp_dir().join(format!("smartdns-encrypted-{}.pem", rand::random::<u64>()));
        fs::write(
            &file,
            encrypted
                .to_pem("ENCRYPTED PRIVATE KEY", pkcs8::LineEnding::LF)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            load_private_key(&file, Some("test password"))
                .unwrap()
                .secret_der(),
            der
        );
        assert!(load_private_key(&file, Some("wrong password")).is_err());
        assert!(load_private_key(&file, None).is_err());
        fs::write(&file, encrypted.as_bytes()).unwrap();
        assert_eq!(
            load_private_key(&file, Some("test password"))
                .unwrap()
                .secret_der(),
            der
        );
        fs::remove_file(file).unwrap();
    }

    #[tokio::test]
    async fn test_spki_pin_accepts_matching_key_and_rejects_other_key() {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let file = std::env::temp_dir().join(format!("smartdns-pin-{}.pem", rand::random::<u64>()));
        fs::write(&file, cert.pem()).unwrap();
        let bundle = TlsClientConfigBundle::new(None, Some(file.clone()));
        let (_, parsed) = x509_parser::parse_x509_certificate(cert.der().as_ref()).unwrap();
        let pin = base64::engine::general_purpose::STANDARD
            .encode(Sha256::digest(parsed.public_key().raw));
        let mismatch = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        for (pin, verify, success) in [
            (&pin, true, true),
            (&mismatch, true, false),
            (&mismatch, false, false),
        ] {
            let server = ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivateKeyDer::try_from(signing_key.serialize_der()).unwrap(),
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let task = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                tokio_rustls::TlsAcceptor::from(Arc::new(server))
                    .accept(stream)
                    .await
            });
            let stream = tokio::net::TcpStream::connect(address).await.unwrap();
            let client = tokio_rustls::TlsConnector::from(
                bundle.for_upstream(verify, true, Some(pin)).unwrap(),
            );
            assert_eq!(
                client
                    .connect("localhost".try_into().unwrap(), stream)
                    .await
                    .is_ok(),
                success
            );
            let _ = task.await;
        }
        fs::remove_file(file).unwrap();
    }
}
