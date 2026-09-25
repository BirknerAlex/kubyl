//! Certificate details for the self-signed certificate interstitial.

use std::time::{Duration, SystemTime};

use sha2::{Digest as _, Sha256};
use x509_cert::Certificate;
use x509_cert::der::Decode as _;
use x509_cert::ext::pkix::SubjectAltName;
use x509_cert::ext::pkix::name::GeneralName;

/// What the interstitial shows about a server certificate.
#[derive(Clone, Debug, PartialEq)]
pub struct CertInfo {
    pub subject: String,
    pub issuer: String,
    pub self_signed: bool,
    pub not_before: String,
    pub not_after: String,
    pub expired: bool,
    pub not_yet_valid: bool,
    /// DNS names and IP addresses the certificate is for.
    pub names: Vec<String>,
    pub sha256: [u8; 32],
}

impl CertInfo {
    /// Parses a DER certificate. Unparsable ones still get a fingerprint.
    pub fn parse(der: &[u8]) -> Self {
        let sha256: [u8; 32] = Sha256::digest(der).into();
        let Ok(cert) = Certificate::from_der(der) else {
            return Self {
                subject: "(unreadable certificate)".into(),
                issuer: String::new(),
                self_signed: false,
                not_before: String::new(),
                not_after: String::new(),
                expired: false,
                not_yet_valid: false,
                names: Vec::new(),
                sha256,
            };
        };
        let tbs = cert.tbs_certificate();
        let validity = tbs.validity();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
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
        Self {
            subject: tbs.subject().to_string(),
            issuer: tbs.issuer().to_string(),
            self_signed: tbs.subject() == tbs.issuer(),
            not_before: validity.not_before.to_string(),
            not_after: validity.not_after.to_string(),
            expired: validity.not_after.to_unix_duration() < now,
            not_yet_valid: validity.not_before.to_unix_duration() > now,
            names,
            sha256,
        }
    }

    /// `5C:9A:37:…` (the way browsers show it).
    pub fn fingerprint(&self) -> String {
        self.sha256
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEM: &str = include_str!("../tests/fixtures/self-signed.pem");

    #[test]
    fn parses_a_self_signed_certificate() {
        let der = crate::native::pem_to_der(PEM).unwrap();
        let info = CertInfo::parse(&der);
        assert!(
            info.subject.contains("CN=argocd-server"),
            "{}",
            info.subject
        );
        assert!(info.self_signed);
        assert!(!info.expired);
        assert!(!info.not_yet_valid);
        assert_eq!(
            info.names,
            ["argocd-server", "argocd-server.argocd.svc", "127.0.0.1"]
        );
        assert!(
            info.not_after.starts_with("2126-09-01"),
            "{}",
            info.not_after
        );
        assert_eq!(
            info.fingerprint(),
            "5C:9A:37:55:A7:D0:4F:90:C6:CD:C2:8F:80:00:4D:92:D3:9F:7A:19:A0:47:AD:59:AD:1E:F3:52:81:19:05:68"
        );
    }

    #[test]
    fn garbage_still_gets_a_fingerprint() {
        let info = CertInfo::parse(b"not a certificate");
        assert_eq!(
            info.sha256,
            <[u8; 32]>::from(Sha256::digest(b"not a certificate"))
        );
        assert!(info.names.is_empty());
    }
}
