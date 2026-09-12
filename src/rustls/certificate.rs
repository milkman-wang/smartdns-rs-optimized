use crate::{
    config::{CertificateGeneration, IBindConfig, SslConfig},
    dns_conf::RuntimeConfig,
};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents)
}

pub fn prepare_server_certificate(
    cfg: &RuntimeConfig,
    ssl: &SslConfig,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    let certificate = ssl.certificate.as_deref().or(cfg.bind_cert_file());
    let key = ssl.certificate_key.as_deref().or(cfg.bind_cert_key_file());
    let generate = match cfg.bind_cert_generate {
        CertificateGeneration::Auto => certificate.is_none() && key.is_none(),
        CertificateGeneration::Yes => true,
        CertificateGeneration::No => false,
    };
    let directory = cfg
        .conf_dir()
        .unwrap_or_else(|| Path::new(crate::dns_conf::DEFAULT_CONF_DIR));
    let certificate = certificate
        .map(Path::to_path_buf)
        .unwrap_or_else(|| directory.join("smartdns-cert.pem"));
    let key = key
        .map(Path::to_path_buf)
        .unwrap_or_else(|| directory.join("smartdns-key.pem"));
    if !generate {
        return Ok((certificate, key));
    }
    if certificate.exists() && key.exists() {
        let certificates = super::load_certs_from_path(&certificate)?;
        if let Some(cert) = certificates.first() {
            let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
                .map_err(|error| anyhow::anyhow!("invalid existing certificate: {error}"))?;
            if parsed.validity().is_valid() {
                return Ok((certificate, key));
            }
        }
    }
    let root_path = cfg
        .bind_cert_root_key_file
        .clone()
        .unwrap_or_else(|| directory.join("smartdns-root-key.pem"));
    let root_key = if root_path.exists() {
        KeyPair::from_pem(&fs::read_to_string(&root_path)?)?
    } else {
        let key = KeyPair::generate()?;
        write_private(&root_path, key.serialize_pem().as_bytes())?;
        key
    };
    let now = time::OffsetDateTime::now_utc();
    let mut root_params = CertificateParams::default();
    root_params
        .distinguished_name
        .push(DnType::CommonName, "SmartDNS Local Root CA");
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    root_params.not_before = now - time::Duration::days(1);
    root_params.not_after = now + time::Duration::days(3650);
    let root_cert = root_params.self_signed(&root_key)?;
    let issuer = Issuer::new(root_params, root_key);

    let mut names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
        cfg.server_name()
            .to_ascii()
            .trim_end_matches('.')
            .to_string(),
    ];
    names.extend(cfg.bind_cert_san.iter().map(|s| {
        s.strip_prefix("DNS:")
            .or_else(|| s.strip_prefix("IP:"))
            .unwrap_or(s)
            .to_string()
    }));
    for listener in cfg.binds() {
        let ip = listener.sock_addr().ip();
        if !ip.is_unspecified() {
            names.push(ip.to_string());
        }
    }
    names.extend(
        local_ip_address::list_afinet_netifas()
            .unwrap_or_default()
            .into_iter()
            .map(|(_, ip)| ip.to_string()),
    );
    names.sort();
    names.dedup();
    let mut params = CertificateParams::new(names)?;
    params
        .distinguished_name
        .push(DnType::CommonName, "SmartDNS");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.use_authority_key_identifier_extension = true;
    params.not_before = now - time::Duration::minutes(5);
    let days = cfg
        .bind_cert_validity_days
        .filter(|days| *days > 0)
        .unwrap_or(390);
    anyhow::ensure!(days <= 9999, "bind-cert-validity-days must be at most 9999");
    params.not_after = now + time::Duration::days(days as i64);
    let server_key = KeyPair::generate()?;
    let server_cert = params.signed_by(&server_key, &issuer)?;
    write_private(&key, server_key.serialize_pem().as_bytes())?;
    write_private(
        &certificate,
        format!("{}{}", server_cert.pem(), root_cert.pem()).as_bytes(),
    )?;
    crate::log::info!(
        "generated TLS certificate {} ({} days)",
        certificate.display(),
        days
    );
    Ok((certificate, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_auto_certificate_chain_san_and_existing_key() {
        let dir = std::env::temp_dir().join(format!("smartdns-cert-{}", rand::random::<u64>()));
        let cfg = RuntimeConfig::builder()
            .with_conf_dir(dir.clone())
            .with("bind-cert-san resolver.example 192.0.2.53 2001:db8::53")
            .with("bind-cert-validity-days 30")
            .build()
            .unwrap();
        let ssl = SslConfig::default();
        let (cert, key) = prepare_server_certificate(&cfg, &ssl).unwrap();
        let original_key = fs::read(&key).unwrap();
        let chain = super::super::load_certs_from_path(&cert).unwrap();
        assert_eq!(chain.len(), 2);
        let (_, parsed) = x509_parser::parse_x509_certificate(chain[0].as_ref()).unwrap();
        assert!(parsed.extensions().iter().any(|extension| matches!(
            extension.parsed_extension(),
            x509_parser::extensions::ParsedExtension::AuthorityKeyIdentifier(_)
        )));
        assert!(
            parsed
                .subject_alternative_name()
                .unwrap()
                .unwrap()
                .value
                .general_names
                .iter()
                .any(|name| matches!(
                    name,
                    x509_parser::extensions::GeneralName::DNSName("resolver.example")
                ))
        );
        let remaining = parsed.validity().not_after.timestamp()
            - time::OffsetDateTime::now_utc().unix_timestamp();
        assert!((29 * 86400..=30 * 86400).contains(&remaining));
        super::super::TlsServerCertResolver::load(&cert, &key, None).unwrap();
        prepare_server_certificate(&cfg, &ssl).unwrap();
        assert_eq!(fs::read(key).unwrap(), original_key);
        fs::remove_dir_all(dir).unwrap();
    }
}
