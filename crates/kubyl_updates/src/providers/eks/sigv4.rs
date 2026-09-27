//! AWS Signature Version 4 (docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv.html) on
//! `hmac` + `sha2`: the canonical request, the string to sign, the signing key (an HMAC-SHA256
//! chain over date, region and service) and the `Authorization` header. Tested with AWS's
//! published examples and a request signed by botocore.

use hmac::{Hmac, KeyInit as _, Mac as _};
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};
use url::Url;

use crate::providers::cloud::encode;

type HmacSha256 = Hmac<Sha256>;

const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// What signs a request. Borrowed from the credentials for as long as one signature takes.
pub struct Keys<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub session_token: Option<&'a str>,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes keys of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// The canonical URI: every path segment URI-encoded once more (services other than S3 encode
/// the already encoded path again).
pub fn canonical_uri(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/".into();
    }
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// The canonical query string: pairs decoded, encoded strictly again and sorted by name, then
/// value.
pub fn canonical_query(query: Option<&str>) -> String {
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return String::new();
    };
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (encode(&decode(name)), encode(&decode(value)))
        })
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-decoding (`+` stays a plus: SigV4 queries are built with `%20`).
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (digit(bytes[i + 1]), digit(bytes[i + 2]))
        {
            out.push(high << 4 | low);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The canonical headers (lowercase names, trimmed values with inner spaces collapsed, sorted)
/// and the signed-headers list.
pub fn canonical_headers(headers: &[(String, String)]) -> (String, String) {
    let mut sorted: Vec<(String, String)> = headers
        .iter()
        .map(|(n, v)| {
            (
                n.to_lowercase(),
                v.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut canonical = String::new();
    let mut names: Vec<String> = Vec::new();
    for (name, value) in sorted {
        if names.last() == Some(&name) {
            // Repeated headers: values joined by commas.
            canonical.pop();
            canonical.push(',');
            canonical.push_str(&value);
            canonical.push('\n');
            continue;
        }
        canonical.push_str(&format!("{name}:{value}\n"));
        names.push(name);
    }
    (canonical, names.join(";"))
}

/// The canonical request and its signed-headers list.
pub fn canonical_request(
    method: &str,
    path: &str,
    query: Option<&str>,
    headers: &[(String, String)],
    payload_hash: &str,
) -> (String, String) {
    let (canonical_headers, signed) = canonical_headers(headers);
    let request = format!(
        "{method}\n{}\n{}\n{canonical_headers}\n{signed}\n{payload_hash}",
        canonical_uri(path),
        canonical_query(query),
    );
    (request, signed)
}

/// `20150830/us-east-1/iam/aws4_request`.
pub fn scope(amz_date: &str, region: &str, service: &str) -> String {
    format!("{}/{region}/{service}/aws4_request", &amz_date[..8])
}

pub fn string_to_sign(amz_date: &str, scope: &str, canonical_request: &str) -> String {
    format!(
        "{ALGORITHM}\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    )
}

/// The signing key: `HMAC(HMAC(HMAC(HMAC("AWS4" + secret, date), region), service),
/// "aws4_request")`.
pub fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    hmac(&k_service, b"aws4_request")
}

pub fn signature(signing_key: &[u8], string_to_sign: &str) -> String {
    hex(&hmac(signing_key, string_to_sign.as_bytes()))
}

/// `20150830T123600Z`.
pub fn amz_date(now: Timestamp) -> String {
    now.strftime("%Y%m%dT%H%M%SZ").to_string()
}

/// The `Host` header of a URL (with the port when it isn't the scheme's default).
pub fn host(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

/// A request to sign.
pub struct Signable<'a> {
    pub method: &'a str,
    pub url: &'a Url,
    /// The other headers to sign (`content-type`).
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
}

/// Signs a request. The answer is the headers to add: `x-amz-date`, `x-amz-security-token`
/// (temporary credentials) and `authorization`.
pub fn sign(
    request: &Signable<'_>,
    keys: &Keys<'_>,
    region: &str,
    service: &str,
    now: Timestamp,
) -> Vec<(&'static str, String)> {
    let Signable {
        method,
        url,
        headers,
        body,
    } = *request;
    let amz_date = amz_date(now);
    let mut signed: Vec<(String, String)> = headers.to_vec();
    signed.push(("host".into(), host(url)));
    signed.push(("x-amz-date".into(), amz_date.clone()));
    if let Some(token) = keys.session_token {
        signed.push(("x-amz-security-token".into(), token.to_string()));
    }
    let (request, signed_headers) =
        canonical_request(method, url.path(), url.query(), &signed, &sha256_hex(body));
    let scope = scope(&amz_date, region, service);
    let key = signing_key(keys.secret_access_key, &amz_date[..8], region, service);
    let signature = signature(&key, &string_to_sign(&amz_date, &scope, &request));
    let mut out = vec![("x-amz-date", amz_date)];
    if let Some(token) = keys.session_token {
        out.push(("x-amz-security-token", token.to_string()));
    }
    out.push((
        "authorization",
        format!(
            "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            keys.access_key_id
        ),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// "Examples of the complete Signature Version 4 signing process" (AWS General Reference):
    /// `GET https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08`.
    #[test]
    fn aws_iam_list_users_example() {
        let headers = vec![
            (
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded; charset=utf-8".to_string(),
            ),
            ("Host".into(), "iam.amazonaws.com".into()),
            ("X-Amz-Date".into(), "20150830T123600Z".into()),
        ];
        let (request, signed) = canonical_request(
            "GET",
            "/",
            Some("Action=ListUsers&Version=2010-05-08"),
            &headers,
            EMPTY,
        );
        assert_eq!(
            request,
            "GET\n/\nAction=ListUsers&Version=2010-05-08\n\
             content-type:application/x-www-form-urlencoded; charset=utf-8\n\
             host:iam.amazonaws.com\nx-amz-date:20150830T123600Z\n\n\
             content-type;host;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(signed, "content-type;host;x-amz-date");
        assert_eq!(
            sha256_hex(request.as_bytes()),
            "f536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59"
        );
        let scope = scope("20150830T123600Z", "us-east-1", "iam");
        assert_eq!(scope, "20150830/us-east-1/iam/aws4_request");
        let to_sign = string_to_sign("20150830T123600Z", &scope, &request);
        assert_eq!(
            to_sign,
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/iam/aws4_request\n\
             f536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59"
        );
        let key = signing_key(SECRET, "20150830", "us-east-1", "iam");
        assert_eq!(
            hex(&key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
        assert_eq!(
            signature(&key, &to_sign),
            "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    /// The whole flow through `sign`, on the same example.
    #[test]
    fn signs_the_iam_example_end_to_end() {
        let url =
            Url::parse("https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08").unwrap();
        let now: Timestamp = "2015-08-30T12:36:00Z".parse().unwrap();
        let headers = sign(
            &Signable {
                method: "GET",
                url: &url,
                headers: &[(
                    "content-type".into(),
                    "application/x-www-form-urlencoded; charset=utf-8".into(),
                )],
                body: b"",
            },
            &Keys {
                access_key_id: "AKIDEXAMPLE",
                secret_access_key: SECRET,
                session_token: None,
            },
            "us-east-1",
            "iam",
            now,
        );
        assert_eq!(headers[0], ("x-amz-date", "20150830T123600Z".to_string()));
        assert_eq!(
            headers[1].1,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, \
             Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    /// The SigV4 test suite's `get-vanilla` and `post-vanilla`.
    #[test]
    fn test_suite_vanilla_requests() {
        for (method, expected) in [
            (
                "GET",
                "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
            ),
            (
                "POST",
                "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b",
            ),
        ] {
            let url = Url::parse("https://example.amazonaws.com/").unwrap();
            let headers = sign(
                &Signable {
                    method,
                    url: &url,
                    headers: &[],
                    body: b"",
                },
                &Keys {
                    access_key_id: "AKIDEXAMPLE",
                    secret_access_key: SECRET,
                    session_token: None,
                },
                "us-east-1",
                "service",
                "2015-08-30T12:36:00Z".parse().unwrap(),
            );
            assert!(
                headers[1].1.ends_with(&format!("Signature={expected}")),
                "{method}: {}",
                headers[1].1
            );
        }
    }

    /// An EKS request with a session token and a JSON body, signed by botocore 1.x
    /// (`awscli.botocore.auth.SigV4Auth`) with the same inputs.
    #[test]
    fn matches_botocore_for_an_eks_write() {
        let url = Url::parse("https://eks.eu-west-1.amazonaws.com/clusters/prod-eu-west-1/updates")
            .unwrap();
        let headers = sign(
            &Signable {
                method: "POST",
                url: &url,
                headers: &[("content-type".into(), "application/json".into())],
                body: br#"{"version":"1.31"}"#,
            },
            &Keys {
                access_key_id: "ASIAEXAMPLEKEYID",
                secret_access_key: SECRET,
                session_token: Some("FwoGZXIvYXdzEXAMPLETOKEN"),
            },
            "eu-west-1",
            "eks",
            "2026-09-26T10:00:00Z".parse().unwrap(),
        );
        assert_eq!(
            headers.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            ["x-amz-date", "x-amz-security-token", "authorization"]
        );
        assert_eq!(headers[2].1, BOTOCORE_EKS_WRITE);
    }

    /// Query strings are sorted and encoded strictly.
    #[test]
    fn matches_botocore_for_a_query() {
        let url = Url::parse(
            "https://eks.eu-west-1.amazonaws.com/addons/supported-versions?kubernetesVersion=1.31&addonName=vpc-cni",
        )
        .unwrap();
        let headers = sign(
            &Signable {
                method: "GET",
                url: &url,
                headers: &[],
                body: b"",
            },
            &Keys {
                access_key_id: "ASIAEXAMPLEKEYID",
                secret_access_key: SECRET,
                session_token: Some("FwoGZXIvYXdzEXAMPLETOKEN"),
            },
            "eu-west-1",
            "eks",
            "2026-09-26T10:00:00Z".parse().unwrap(),
        );
        assert_eq!(headers[2].1, BOTOCORE_EKS_QUERY);
    }

    #[test]
    fn canonical_parts() {
        assert_eq!(canonical_uri(""), "/");
        assert_eq!(
            canonical_uri("/clusters/prod/node-groups/ng-1"),
            "/clusters/prod/node-groups/ng-1"
        );
        // Already encoded segments are encoded again.
        assert_eq!(canonical_uri("/a%20b"), "/a%2520b");
        assert_eq!(
            canonical_query(Some("b=2&a=z&a=y&c=a%20b")),
            "a=y&a=z&b=2&c=a%20b"
        );
        assert_eq!(canonical_query(Some("")), "");
        let (headers, signed) = canonical_headers(&[
            ("X-Amz-Date".into(), "20150830T123600Z".into()),
            ("My-Header".into(), "  a   b  ".into()),
            ("my-header".into(), "c".into()),
        ]);
        assert_eq!(headers, "my-header:a b,c\nx-amz-date:20150830T123600Z\n");
        assert_eq!(signed, "my-header;x-amz-date");
    }

    const BOTOCORE_EKS_WRITE: &str = "AWS4-HMAC-SHA256 Credential=ASIAEXAMPLEKEYID/20260926/eu-west-1/eks/aws4_request, SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, Signature=b59abd2e266c69028a2e8f0d5cb21eeba62d8efa72f1627334907218fe93b8d9";
    const BOTOCORE_EKS_QUERY: &str = "AWS4-HMAC-SHA256 Credential=ASIAEXAMPLEKEYID/20260926/eu-west-1/eks/aws4_request, SignedHeaders=host;x-amz-date;x-amz-security-token, Signature=c09e6461f192e1f32475d535dfe86a4ec12dbf4ebeb5bd0b9272a6104197bd32";
}
