//! The index document (`index.json`, format 1). The contract is owned by the
//! oyzu client: `docs/contracts/tools-v1/tool-index.schema.json` (oyzu PR
//! #186). This is the producer's model of it and must stay field-for-field
//! compatible, because the client parses with `deny_unknown_fields`.
//!
//! Artifact-set records are carried as the JSON `oyzu tools evidence`
//! emitted, untouched, so their digests still recompute.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

pub const FORMAT: u32 = 1;
pub const KIND: &str = "tool-index";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub format: u32,
    pub kind: String,
    /// Release counter, stamped on the published copy only (see
    /// `crate::stamp`). The committed `index/index.json` never carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<u64>,
    /// RFC 3339 UTC expiry, stamped with `serial`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    /// The `engine` object of the artifact-set documents the records came from.
    pub engine: Value,
    pub snapshot: Snapshot,
    pub tools: Vec<Tool>,
}

/// The same object as an artifact-set document's `snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub source: String,
    pub tag: Option<String>,
    pub commit: Option<String>,
    pub form: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    /// Canonical id: `oyzu:<name>` or `oyzu:<owner>/<repo>`.
    pub id: String,
    /// Where the records came from (`aqua:<package>`, `core:<provider>`);
    /// every record's `tool.id` equals it.
    pub origin: String,
    pub serving_class: ServingClass,
    /// Newest first.
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum ServingClass {
    Cache,
    CacheSourceAvailable,
    Direct,
    Excluded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Version {
    /// Normalized: no tag prefix, no leading `v`; equals each record's
    /// `version.normalized`.
    pub version: String,
    pub released: Released,
    /// One record per platform, sorted by platform. Empty for tools at the
    /// discovery tier and for versions whose records have aged out.
    pub sets: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Released {
    pub time: String,
    pub source: DateSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DateSource {
    /// The publisher dated the release.
    Publisher,
    /// The first job run that saw the version (decided 2026-10-10).
    FirstSeen,
}

/// `oyzu:<name>` or `oyzu:<owner>/<repo>[/<name>]`: at most three lowercase
/// segments of `[a-z0-9._-]`, as the JSON schema requires.
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("oyzu:").is_some_and(|rest| {
        let segments: Vec<&str> = rest.split('/').collect();
        segments.len() <= 3
            && segments.iter().all(|s| {
                !s.is_empty()
                    && *s != "."
                    && *s != ".."
                    && s.bytes().all(|b| {
                        b.is_ascii_lowercase()
                            || b.is_ascii_digit()
                            || matches!(b, b'.' | b'_' | b'-')
                    })
            })
    })
}

impl Index {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let index: Index = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        index.check()?;
        Ok(index)
    }

    /// The client's structural rules, so the job never publishes a file the
    /// client would refuse. Digest recomputation is left to the client and to
    /// `oyzu tools evidence`, which produced the records.
    pub fn check(&self) -> Result<()> {
        ensure!(
            self.format == FORMAT && self.kind == KIND,
            "not a format-1 tool index"
        );
        let mut previous: Option<&str> = None;
        for tool in &self.tools {
            ensure!(valid_id(&tool.id), "{:?} is not an oyzu: tool id", tool.id);
            ensure!(
                previous.is_none_or(|p| p < tool.id.as_str()),
                "tools are not sorted by id, or {} is listed twice",
                tool.id
            );
            previous = Some(&tool.id);
            ensure!(
                tool.serving_class != ServingClass::Excluded,
                "{} is excluded and must not be indexed",
                tool.id
            );
            let mut seen = BTreeSet::new();
            for v in &tool.versions {
                ensure!(
                    !v.version.is_empty() && seen.insert(&v.version),
                    "{} lists {:?} twice or empty",
                    tool.id,
                    v.version
                );
                let mut platform: Option<&str> = None;
                for r in &v.sets {
                    let p = r["platform"].as_str().unwrap_or_default();
                    ensure!(
                        platform.is_none_or(|q| q < p),
                        "{} {} records are not sorted by platform",
                        tool.id,
                        v.version
                    );
                    platform = Some(p);
                    ensure!(
                        r["tool"]["id"] == tool.origin.as_str()
                            && r["version"]["normalized"] == v.version.as_str(),
                        "a {} {} record names {} {}",
                        tool.id,
                        v.version,
                        r["tool"]["id"],
                        r["version"]["normalized"]
                    );
                }
            }
        }
        Ok(())
    }

    /// Stable bytes: two-space indentation and a trailing newline, so the
    /// reviewed pull request shows a readable diff.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = serde_json::to_vec_pretty(self)?;
        out.push(b'\n');
        Ok(out)
    }

    /// Whether this is a published copy carrying `serial` or `expires`.
    pub fn stamped(&self) -> bool {
        self.serial.is_some() || self.expires.is_some()
    }

    /// The same document without `serial` and `expires`: what review compares.
    pub fn unstamped(&self) -> Index {
        Index {
            serial: None,
            expires: None,
            ..self.clone()
        }
    }

    pub fn by_origin(&self, origin: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.origin == origin)
    }

    pub fn tool(&self, id: &str) -> Option<&Tool> {
        self.tools
            .binary_search_by(|t| t.id.as_str().cmp(id))
            .ok()
            .map(|i| &self.tools[i])
    }
}

/// A pre-release version (`1.2.0-rc.1`), as the client decides it.
pub fn prerelease(version: &str) -> bool {
    let core = word_prefix(version).map_or(version, |(_, rest)| rest);
    !date_version(core) && core.contains('-')
}

/// A version spelled `WORD.<number>…`, as MinIO tags every release
/// (`RELEASE.2025-08-13T08-35-41Z`): the word and the rest.
pub fn word_prefix(version: &str) -> Option<(&str, &str)> {
    let (word, rest) = version.split_once('.')?;
    (!word.is_empty()
        && word.bytes().all(|b| b.is_ascii_alphabetic())
        && rest.bytes().next().is_some_and(|b| b.is_ascii_digit()))
    .then_some((word, rest))
}

/// A release named by its date, `2025-08-13` or `2025-08-13T08-35-41Z`,
/// whose hyphens are not a pre-release suffix.
fn date_version(v: &str) -> bool {
    let b = v.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    b.len() >= 10
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && (b.len() == 10
            || (b[10] == b'T'
                && b[11..]
                    .iter()
                    .all(|c| c.is_ascii_digit() || matches!(c, b'-' | b':' | b'Z'))))
}

/// Whether a version's bytes were checked: a verified record, or only
/// `unsupported` records (nothing to download).
pub fn verified(v: &Version) -> bool {
    v.sets.iter().any(|r| r["tier"] == "verified")
        || (!v.sets.is_empty() && v.sets.iter().all(|r| r["status"] == "unsupported"))
}

/// Every https host a record names, for the review rule on new download hosts.
pub fn record_hosts(record: &Value) -> BTreeSet<String> {
    record["urls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| u["url"].as_str())
        .filter_map(|u| url::Url::parse(u).ok())
        .filter_map(|u| u.host_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn dated_releases_are_not_prereleases() {
        use super::prerelease;
        assert!(prerelease("1.3.0-rc.1"));
        assert!(!prerelease("1.3.0"));
        assert!(!prerelease("RELEASE.2025-08-13T08-35-41Z"));
        assert!(!prerelease("2025-08-13T08-35-41Z"));
        assert!(!prerelease("2025-08-13"));
        assert!(prerelease("2025-08-13-beta"));
        assert!(prerelease("RELEASE.1.0-rc1"));
    }

    use super::*;

    #[test]
    fn ids() {
        assert!(valid_id("oyzu:terraform"));
        assert!(valid_id("oyzu:kubernetes/kubernetes/kubectl"));
        assert!(valid_id("oyzu:burntsushi/ripgrep"));
        assert!(!valid_id("oyzu:BurntSushi/ripgrep"));
        assert!(!valid_id("oyzu:burntsushi/RipGrep"));
        assert!(!valid_id("aqua:cli/cli"));
        assert!(!valid_id("oyzu:a/b/c/d"));
        assert!(!valid_id("oyzu:../x"));
    }
}
