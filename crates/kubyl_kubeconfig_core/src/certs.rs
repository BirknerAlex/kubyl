//! Certificates and keys in kubeconfigs: PEM blocks, subject, issuer, validity, fingerprints.

use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use x509_cert::Certificate;
use x509_cert::der::{Decode as _, Encode as _};
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{BasicConstraints, SubjectAltName};

/// Certificates expiring within this many days are flagged.
pub const EXPIRY_WARNING_DAYS: i64 = 30;

/// A PEM block: its label (`CERTIFICATE`, `RSA PRIVATE KEY`…) and DER bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PemBlock {
    pub label: String,
    pub der: Vec<u8>,
}

/// The PEM blocks in `text` (headers inside a block are skipped).
pub fn pem_blocks(text: &str) -> Vec<PemBlock> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        let Some(label) = line
            .strip_prefix("-----BEGIN ")
            .and_then(|l| l.strip_suffix("-----"))
        else {
            continue;
        };
        let end = format!("-----END {label}-----");
        let mut body = String::new();
        for line in lines.by_ref() {
            let line = line.trim();
            if line == end {
                break;
            }
            if !line.contains(':') {
                body.push_str(line);
            }
        }
        if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(body) {
            out.push(PemBlock {
                label: label.to_string(),
                der,
            });
        }
    }
    out
}

/// PEM text as pasted: also accepts PEM whose line breaks were lost (a one-line input).
pub fn normalize_pem(text: &str) -> String {
    let text = text.trim();
    if text.contains('\n') || !text.starts_with("-----BEGIN ") {
        return text.to_string();
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN ") {
        let after = &rest[start + 11..];
        let Some(label_end) = after.find("-----") else {
            break;
        };
        let label = &after[..label_end];
        let body_start = start + 11 + label_end + 5;
        let end_marker = format!("-----END {label}-----");
        let Some(end) = rest[body_start..].find(&end_marker) else {
            break;
        };
        let body: String = rest[body_start..body_start + end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        out.push_str(&der_to_pem_text(label, &body));
        rest = &rest[body_start + end + end_marker.len()..];
    }
    if out.is_empty() {
        text.to_string()
    } else {
        out
    }
}

fn der_to_pem_text(label: &str, base64: &str) -> String {
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in base64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// PEM text from a kubeconfig `*-data` value (base64 of PEM). Also accepts raw PEM pasted by
/// mistake.
pub fn pem_from_data(data: &str) -> Result<String, String> {
    let trimmed = data.trim();
    if trimmed.starts_with("-----BEGIN") {
        return Ok(trimmed.to_string());
    }
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|_| "not valid base64".to_string())?;
    String::from_utf8(bytes).map_err(|_| "the decoded data isn't PEM text".to_string())
}

/// The `*-data` value for PEM text.
pub fn data_from_pem(pem: &str) -> String {
    let mut pem = pem.trim().to_string();
    pem.push('\n');
    base64::engine::general_purpose::STANDARD.encode(pem)
}

/// What the UI shows about a certificate.
#[derive(Clone, Debug, PartialEq)]
pub struct CertInfo {
    pub subject: String,
    pub issuer: String,
    pub self_signed: bool,
    pub is_ca: bool,
    pub not_before: jiff::Timestamp,
    pub not_after: jiff::Timestamp,
    /// DNS names and IPs.
    pub names: Vec<String>,
    /// SHA-256 of the DER certificate.
    pub sha256: [u8; 32],
    /// SHA-256 of the DER SubjectPublicKeyInfo (kubeadm's `--discovery-token-ca-cert-hash`).
    pub spki_sha256: [u8; 32],
    pub der: Vec<u8>,
}

impl CertInfo {
    pub fn parse(der: &[u8]) -> Result<Self, String> {
        let cert = Certificate::from_der(der).map_err(|e| format!("not a certificate: {e}"))?;
        let tbs = cert.tbs_certificate();
        let validity = tbs.validity();
        let ts = |t: &x509_cert::time::Time| {
            jiff::Timestamp::from_second(t.to_unix_duration().as_secs() as i64)
                .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        };
        let is_ca = matches!(
            tbs.get_extension::<BasicConstraints>(),
            Ok(Some((_, BasicConstraints { ca: true, .. })))
        );
        let names = match tbs.get_extension::<SubjectAltName>() {
            Ok(Some((_, san))) => san
                .0
                .iter()
                .filter_map(|name| match name {
                    GeneralName::DnsName(dns) => Some(dns.to_string()),
                    GeneralName::IpAddress(ip) => match ip.as_bytes() {
                        [a, b, c, d] => Some(std::net::Ipv4Addr::new(*a, *b, *c, *d).to_string()),
                        bytes if bytes.len() == 16 => {
                            let mut octets = [0u8; 16];
                            octets.copy_from_slice(bytes);
                            Some(std::net::Ipv6Addr::from(octets).to_string())
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let spki = tbs
            .subject_public_key_info()
            .to_der()
            .map_err(|e| format!("unreadable public key: {e}"))?;
        Ok(Self {
            subject: tbs.subject().to_string(),
            issuer: tbs.issuer().to_string(),
            self_signed: tbs.subject() == tbs.issuer(),
            is_ca,
            not_before: ts(&validity.not_before),
            not_after: ts(&validity.not_after),
            names,
            sha256: Sha256::digest(der).into(),
            spki_sha256: Sha256::digest(&spki).into(),
            der: der.to_vec(),
        })
    }

    /// `5C:9A:37:…`.
    pub fn fingerprint(&self) -> String {
        colon_hex(&self.sha256)
    }

    /// `sha256:<hex>` like kubeadm prints it.
    pub fn spki_hash(&self) -> String {
        format!(
            "sha256:{}",
            self.spki_sha256
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    }

    pub fn expired(&self) -> bool {
        self.not_after < jiff::Timestamp::now()
    }

    /// Days until it expires (negative when expired).
    pub fn days_left(&self) -> i64 {
        (self.not_after.as_second() - jiff::Timestamp::now().as_second()).div_euclid(86_400)
    }

    pub fn expires_soon(&self) -> bool {
        !self.expired() && self.days_left() < EXPIRY_WARNING_DAYS
    }

    /// `2026-11-02`.
    pub fn not_after_date(&self) -> String {
        self.not_after.strftime("%Y-%m-%d").to_string()
    }

    /// The certificate as PEM.
    pub fn pem(&self) -> String {
        der_to_pem("CERTIFICATE", &self.der)
    }

    /// The `CN=` of the subject, or the whole subject.
    pub fn common_name(&self) -> String {
        common_name(&self.subject)
    }
}

/// `CN=kubernetes,O=x` → `kubernetes`.
pub fn common_name(name: &str) -> String {
    name.split(',')
        .find_map(|part| part.trim().strip_prefix("CN="))
        .unwrap_or(name)
        .to_string()
}

pub fn colon_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// PEM text for DER bytes.
pub fn der_to_pem(label: &str, der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// Every certificate in PEM text. Errors when there is none (or one is unreadable).
pub fn certificates(pem: &str) -> Result<Vec<CertInfo>, String> {
    let blocks: Vec<PemBlock> = pem_blocks(pem)
        .into_iter()
        .filter(|b| b.label == "CERTIFICATE")
        .collect();
    if blocks.is_empty() {
        return Err("no certificate found (expected a PEM \"BEGIN CERTIFICATE\" block)".into());
    }
    blocks.iter().map(|b| CertInfo::parse(&b.der)).collect()
}

/// Checks that PEM text holds a private key (never returns or logs the key).
pub fn check_private_key(pem: &str) -> Result<(), String> {
    let found = pem_blocks(pem)
        .iter()
        .any(|b| b.label.ends_with("PRIVATE KEY") && !b.der.is_empty());
    if found {
        Ok(())
    } else {
        Err("no private key found (expected a PEM \"PRIVATE KEY\" block)".into())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A CA and a server certificate for `127.0.0.1` and `localhost` it signed, plus a client
    /// certificate and key (generated with openssl for these tests; validity 100 years).
    pub const CA: &str = include_str!("../tests/fixtures/ca.pem");
    pub const OTHER_CA: &str = include_str!("../tests/fixtures/other-ca.pem");
    pub const CLIENT: &str = include_str!("../tests/fixtures/client.pem");
    pub const CLIENT_KEY: &str = include_str!("../tests/fixtures/client-key.pem");

    #[test]
    fn parses_certificates_and_keys() {
        let ca = certificates(CA).unwrap().remove(0);
        assert_eq!(ca.common_name(), "kubyl-test-ca");
        assert!(ca.is_ca && ca.self_signed && !ca.expired() && !ca.expires_soon());
        assert!(ca.spki_hash().starts_with("sha256:"));
        assert_eq!(ca.fingerprint().len(), 32 * 3 - 1);
        let client = certificates(CLIENT).unwrap().remove(0);
        assert_eq!(client.common_name(), "kubernetes-admin");
        assert!(!client.is_ca && !client.self_signed);
        assert!(check_private_key(CLIENT_KEY).is_ok());
        assert!(check_private_key(CLIENT).is_err());
        assert!(certificates("junk").is_err());
        // Round trips through the kubeconfig's base64 form.
        let data = data_from_pem(CA);
        assert_eq!(pem_from_data(&data).unwrap().trim(), CA.trim());
        assert_eq!(pem_from_data(CA).unwrap().trim(), CA.trim());
        assert_eq!(certificates(&ca.pem()).unwrap()[0].sha256, ca.sha256);
        // A PEM pasted into a one-line input.
        let flat = CLIENT_KEY.replace('\n', "");
        assert!(check_private_key(&normalize_pem(&flat)).is_ok());
        assert_eq!(normalize_pem(CA), CA.trim());
    }
}
