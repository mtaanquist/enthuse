//! Platform-independent paths inside a source.

use std::fmt;

/// A path inside a [`Source`](super::Source), relative to its root.
///
/// Segments are joined with `/` on every platform, and `.`/`..` or empty
/// segments are rejected, so a `SourcePath` can never escape its source and
/// is stable as a database key. The root is the empty path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SourcePath(String);

/// A path segment that is empty, `.`, `..` or contains a separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidPath(pub String);

impl fmt::Display for InvalidPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid path segment '{}'", self.0)
    }
}

impl std::error::Error for InvalidPath {}

fn check_segment(segment: &str) -> Result<(), InvalidPath> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.contains(['/', '\\', '\0'])
    {
        Err(InvalidPath(segment.to_owned()))
    } else {
        Ok(())
    }
}

impl SourcePath {
    /// The root of the source.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Parses a stored `/`-separated path. Leading and trailing `/` are
    /// ignored.
    pub fn parse(s: &str) -> Result<Self, InvalidPath> {
        let trimmed = s.trim_matches('/');
        if trimmed.is_empty() {
            return Ok(Self::root());
        }
        trimmed.split('/').try_for_each(check_segment)?;
        Ok(Self(trimmed.to_owned()))
    }

    /// Appends one segment (a file or folder name).
    pub fn join(&self, segment: &str) -> Result<Self, InvalidPath> {
        check_segment(segment)?;
        Ok(if self.is_root() {
            Self(segment.to_owned())
        } else {
            Self(format!("{}/{}", self.0, segment))
        })
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The containing folder, or `None` for the root.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(match self.0.rsplit_once('/') {
            Some((parent, _)) => Self(parent.to_owned()),
            None => Self::root(),
        })
    }

    /// The last segment, or `None` for the root.
    pub fn name(&self) -> Option<&str> {
        if self.is_root() {
            None
        } else {
            self.0.rsplit('/').next()
        }
    }

    /// The segments from the root down.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|s| !s.is_empty())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_parent_name_roundtrip() {
        let p = SourcePath::root()
            .join("TV")
            .unwrap()
            .join("Season 1")
            .unwrap();
        assert_eq!(p.as_str(), "TV/Season 1");
        assert_eq!(p.name(), Some("Season 1"));
        assert_eq!(p.parent().unwrap().as_str(), "TV");
        assert_eq!(p.parent().unwrap().parent(), Some(SourcePath::root()));
        assert_eq!(SourcePath::root().parent(), None);
        assert_eq!(SourcePath::parse(p.as_str()).unwrap(), p);
        assert_eq!(p.segments().collect::<Vec<_>>(), ["TV", "Season 1"]);
        assert_eq!(SourcePath::root().segments().count(), 0);
    }

    #[test]
    fn rejects_escaping_and_separators() {
        let root = SourcePath::root();
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert!(root.join(bad).is_err(), "{bad:?}");
        }
        assert!(SourcePath::parse("a/../b").is_err());
        assert!(SourcePath::parse("a//b").is_err());
        assert_eq!(SourcePath::parse("/a/b/").unwrap().as_str(), "a/b");
        assert!(SourcePath::parse("").unwrap().is_root());
    }
}
