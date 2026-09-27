//! Version strings of the providers: `4.17.8`, `v1.30.4-eks-1a2b3c`, `1.30`, `v1.33.4+k3s1`,
//! `1.30.5-gke.1014001`. Compared by their numbers; the suffix only breaks ties.

use std::cmp::Ordering;
use std::fmt;

/// A parsed version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: Option<u64>,
    /// What follows the numbers (`-eks-1a2b3c`, `+k3s1`, `-gke.1014001`).
    pub suffix: String,
}

impl Version {
    /// Parses a version, with or without a leading `v`. `None` for anything without at least
    /// `major.minor`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let end = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(text.len());
        let (numbers, suffix) = text.split_at(end);
        let mut parts = numbers.split('.').filter(|p| !p.is_empty());
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next().and_then(|p| p.parse().ok());
        Some(Self {
            major,
            minor,
            patch,
            suffix: suffix.to_string(),
        })
    }

    /// `(major, minor)`.
    pub fn minor_key(&self) -> (u64, u64) {
        (self.major, self.minor)
    }

    /// `1.30`.
    pub fn minor_string(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }

    /// Minors between `self` and `other` (positive when `other` is newer, same major only).
    pub fn minors_to(&self, other: &Version) -> i64 {
        if self.major != other.major {
            return if other.major > self.major {
                i64::MAX
            } else {
                i64::MIN
            };
        }
        other.minor as i64 - self.minor as i64
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if let Some(patch) = self.patch {
            write!(f, ".{patch}")?;
        }
        f.write_str(&self.suffix)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.unwrap_or(0).cmp(&other.patch.unwrap_or(0)))
            // `-rc.3`, `-ec.1`, `-alpha`… come before the release itself.
            .then_with(|| is_prerelease(&other.suffix).cmp(&is_prerelease(&self.suffix)))
            .then_with(|| suffix_numbers(&self.suffix).cmp(&suffix_numbers(&other.suffix)))
    }
}

/// A pre-release suffix (OpenShift's candidate channels list `-ec.N` and `-rc.N` builds).
fn is_prerelease(suffix: &str) -> bool {
    let suffix = suffix.trim_start_matches(['-', '.']).to_ascii_lowercase();
    ["rc", "ec", "alpha", "beta", "pre"]
        .iter()
        .any(|p| suffix.starts_with(p))
}

/// The numbers in a suffix (`+k3s2` → `[3, 2]`, `-gke.1014001` → `[1014001]`).
fn suffix_numbers(suffix: &str) -> Vec<u64> {
    suffix
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// Compares two version strings; unparsable ones sort first, then by text.
pub fn compare(a: &str, b: &str) -> Ordering {
    match (Version::parse(a), Version::parse(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => a.cmp(b),
    }
}

/// `(major, minor)` of a version string.
pub fn minor_of(text: &str) -> Option<(u64, u64)> {
    Version::parse(text).map(|v| v.minor_key())
}

/// The Kubernetes minor an OpenShift minor ships (4.y ships 1.(y+13)).
pub fn openshift_kube_minor(openshift: &Version) -> Option<(u64, u64)> {
    (openshift.major == 4).then_some((1, openshift.minor + 13))
}

/// The next minor: `1.30.4` → `1.31`.
pub fn next_minor(text: &str) -> Option<String> {
    let v = Version::parse(text)?;
    Some(format!("{}.{}", v.major, v.minor + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_versions() {
        let v = Version::parse("v1.30.4-eks-1a2b3c").unwrap();
        assert_eq!((v.major, v.minor, v.patch), (1, 30, Some(4)));
        assert_eq!(v.suffix, "-eks-1a2b3c");
        assert_eq!(Version::parse("1.30").unwrap().patch, None);
        assert_eq!(Version::parse("v1.33.4+k3s1").unwrap().suffix, "+k3s1");
        assert_eq!(Version::parse("4.17.8").unwrap().to_string(), "4.17.8");
        assert!(Version::parse("latest").is_none());
        assert!(Version::parse("v1").is_none());
    }

    #[test]
    fn compares_numbers_then_suffixes() {
        assert_eq!(compare("4.17.10", "4.17.9"), Ordering::Greater);
        assert_eq!(compare("v1.33.4+k3s1", "v1.33.4+k3s2"), Ordering::Less);
        assert_eq!(compare("1.30", "1.30.0"), Ordering::Equal);
        assert_eq!(
            compare("1.30.5-gke.1014001", "1.30.5-gke.1162000"),
            Ordering::Less
        );
        assert_eq!(compare("4.18.0", "4.17.99"), Ordering::Greater);
        // Pre-releases sort before their release, and among themselves by number.
        assert_eq!(compare("4.18.0-rc.3", "4.18.0"), Ordering::Less);
        assert_eq!(compare("4.18.0-ec.2", "4.18.0-ec.1"), Ordering::Greater);
        assert_eq!(compare("4.18.0-rc.1", "4.17.12"), Ordering::Greater);
        // k3s/GKE build suffixes aren't pre-releases.
        assert_eq!(compare("v1.33.4+k3s2", "v1.33.4"), Ordering::Greater);
    }

    #[test]
    fn minors() {
        let a = Version::parse("1.30.4").unwrap();
        let b = Version::parse("1.32").unwrap();
        assert_eq!(a.minors_to(&b), 2);
        assert_eq!(next_minor("v1.37.0").as_deref(), Some("1.38"));
        assert_eq!(
            openshift_kube_minor(&Version::parse("4.17.8").unwrap()),
            Some((1, 30))
        );
    }
}
