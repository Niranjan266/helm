//! Semantic version parsing and precedence (semver.org 2.0.0, section 11).

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// One dot-separated pre-release identifier. Numeric identifiers sort below
/// alphanumeric ones (variant order matters for the derived `Ord`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PreId {
    Num(u64),
    Alpha(String),
}

/// A parsed semantic version. Build metadata is accepted but ignored for
/// ordering and equality, as the spec requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<PreId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid version {0:?}")]
pub struct VersionError(pub String);

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: Vec::new(),
        }
    }

    /// Parses `1.2.3`, `v1.2.3`, `1.2.3-beta.1`, `1.2.3+build.5`.
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let err = || VersionError(text.to_string());
        let s = text.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let s = s
            .split_once('+')
            .map_or(s, |(core, build)| if build.is_empty() { "" } else { core });
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };

        let mut parts = core.split('.');
        let mut next = || parts.next().and_then(parse_num).ok_or_else(err);
        let (major, minor, patch) = (next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(err());
        }

        let pre = match pre {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(|id| parse_pre_id(id).ok_or_else(err))
                .collect::<Result<_, _>>()?,
        };
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// True for a release version (no pre-release identifiers).
    pub fn is_stable(&self) -> bool {
        self.pre.is_empty()
    }
}

fn parse_num(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

fn parse_pre_id(s: &str) -> Option<PreId> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        parse_num(s).map(PreId::Num)
    } else {
        Some(PreId::Alpha(s.to_string()))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Lexicographic over identifiers; a shorter prefix sorts first.
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for Version {
    type Err = VersionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, id) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match id {
                PreId::Num(n) => write!(f, "{n}")?,
                PreId::Alpha(a) => f.write_str(a)?,
            }
        }
        Ok(())
    }
}
