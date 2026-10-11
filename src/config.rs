//! The job's checked-in configuration: `config/index.toml` (pins and limits),
//! `config/launch-set.toml` (the verified tier) and `config/serving.toml`
//! (the people-maintained license and serving-class supplement), plus the
//! optional `config/withdrawn.toml` (versions withdrawn from the index).
use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexConfig {
    pub aqua_registry: RegistryPin,
    pub names: NamesPin,
    pub platforms: Vec<String>,
    pub discovery: Discovery,
    pub verified: Verified,
    pub signing: Signing,
    #[serde(default)]
    pub core: BTreeMap<String, CoreProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryPin {
    pub source: String,
    pub tag: String,
    pub commit: Option<String>,
    pub digest: String,
    /// `{tag}` is replaced.
    pub registry_url: String,
    pub license_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamesPin {
    pub source: String,
    pub version: String,
    pub digest: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    /// Newest versions kept per tool.
    pub max_versions: usize,
    /// Source kinds indexed at the discovery tier.
    pub source_kinds: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    /// Newest versions verified when a tool joins the launch set.
    pub backfill: usize,
    /// Most (tool, version) pairs verified in one run; the rest wait a day.
    pub max_versions_per_run: usize,
    /// Versions per tool whose records the index keeps, newest first.
    pub keep_records: usize,
    pub include_prereleases: bool,
    /// Days a version that failed verification waits before it is tried
    /// again with the same oyzu (`index/attempts.json`).
    pub retry_after_days: i64,
    /// Platforms the job's oyzu installs core providers on that an earlier
    /// oyzu did not. A kept core-provider version whose record says oyzu does
    /// not install it there (`PLATFORM_UNSUPPORTED`) is verified again, once
    /// per oyzu revision (`index/attempts.json` remembers the check).
    #[serde(default)]
    pub recheck_platforms: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signing {
    pub expires_days: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreProvider {
    pub names: Vec<String>,
    pub max_versions: usize,
    /// Verify new versions like a launch-set tool: `oyzu tools evidence
    /// --provider <name>` builds and checks their records. Otherwise the
    /// provider is listed at the discovery tier (versions and dates only).
    #[serde(default)]
    pub verified: bool,
    /// Leading version components that name a release line (`1`: Node 22,
    /// `2`: Go 1.25). With it, `max_versions`, the backfill and the records
    /// kept count per line, so every supported line keeps verified records,
    /// not only the newest releases overall.
    #[serde(default)]
    pub line: Option<usize>,
    /// The newest release lines kept (every listed line when unset). Needs `line`.
    #[serde(default)]
    pub lines: Option<usize>,
    /// Versions per line whose records the index keeps (default
    /// `verified.keep_records`). Needs `line`.
    #[serde(default)]
    pub keep_records: Option<usize>,
}

impl CoreProvider {
    /// The release line of a version, when the provider counts by line.
    pub fn line_of<'a>(&self, version: &'a str) -> Option<&'a str> {
        let n = self.line?;
        let end = version
            .match_indices('.')
            .nth(n - 1)
            .map_or(version.len(), |(i, _)| i);
        Some(&version[..end])
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSet {
    #[serde(rename = "tool")]
    pub tools: Vec<LaunchTool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchTool {
    pub package: String,
    /// The curated short name (`oyzu:<name>`), when the tool has one.
    #[serde(default)]
    pub name: Option<String>,
    /// For readers of the file; the job does not use it.
    #[allow(dead_code)]
    pub group: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingSupplement {
    /// Keyed by tool id (`aqua:cli/cli`, `core:java`).
    #[serde(default)]
    pub tools: BTreeMap<String, ServingEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingEntry {
    /// SPDX expression as the publisher declares it.
    pub license: String,
    /// Overrides the class the license maps to.
    pub class: Option<String>,
    /// Only a reviewed entry changes the class. Unreviewed entries are
    /// proposals: the tool stays `direct` and the license is shown.
    pub reviewed: bool,
    /// Where the source archive is, for a copyleft tool served from Oyzu's
    /// host: `github-archive`, `adoptium`, or a URL with `{tag}`, `{version}`
    /// and `{repository}`. Defaults to GitHub's tag archive for an aqua
    /// package and Adoptium for `core:java` (see `crate::sources`).
    #[serde(default)]
    pub source: Option<String>,
    /// For readers of the file; the job does not use it.
    #[serde(default)]
    #[allow(dead_code)]
    pub note: Option<String>,
}

/// `config/withdrawn.toml`: versions people withdrew from the index. A
/// withdrawn version stays listed, with its records and the reason, so
/// clients can refuse it by name.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Withdrawals {
    #[serde(default, rename = "version")]
    pub versions: Vec<Withdrawal>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Withdrawal {
    /// Canonical index id (`oyzu:golang-migrate`).
    pub tool: String,
    /// Normalized version, as the index lists it.
    pub version: String,
    /// Shown to people the client refuses; at most 512 characters.
    pub reason: String,
}

impl Withdrawals {
    /// Reads and checks the file; a missing file withdraws nothing.
    pub fn read(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let w: Self = read_toml(path)?;
        w.check()
            .with_context(|| format!("checking {}", path.display()))?;
        Ok(w)
    }

    fn check(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for w in &self.versions {
            ensure!(
                crate::index::valid_id(&w.tool),
                "{:?} is not an oyzu: tool id",
                w.tool
            );
            ensure!(!w.version.is_empty(), "{} has an empty version", w.tool);
            ensure!(
                crate::index::valid_withdrawn_reason(&w.reason),
                "{} {}: the reason must be 1 to {} characters",
                w.tool,
                w.version,
                crate::index::MAX_WITHDRAWN_REASON
            );
            ensure!(
                seen.insert((&w.tool, &w.version)),
                "{} {} is withdrawn twice",
                w.tool,
                w.version
            );
        }
        Ok(())
    }

    /// Withdrawals naming a tool or version `index` does not list, as
    /// `<tool> <version>`.
    pub fn not_indexed(&self, index: &crate::index::Index) -> Vec<String> {
        self.versions
            .iter()
            .filter(|w| {
                index
                    .tool(&w.tool)
                    .is_none_or(|t| !t.versions.iter().any(|v| v.version == w.version))
            })
            .map(|w| format!("{} {}", w.tool, w.version))
            .collect()
    }

    /// Reasons by tool id, then version.
    pub fn by_tool(&self) -> BTreeMap<&str, BTreeMap<&str, &str>> {
        let mut out: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
        for w in &self.versions {
            out.entry(&w.tool)
                .or_default()
                .insert(&w.version, &w.reason);
        }
        out
    }
}

pub struct Config {
    pub index: IndexConfig,
    pub launch_set: LaunchSet,
    pub serving: ServingSupplement,
    pub withdrawn: Withdrawals,
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

impl Config {
    pub fn load(dir: &Path) -> Result<Self> {
        let config = Config {
            index: read_toml(&dir.join("index.toml"))?,
            launch_set: read_toml(&dir.join("launch-set.toml"))?,
            serving: read_toml(&dir.join("serving.toml"))?,
            withdrawn: Withdrawals::read(&dir.join("withdrawn.toml"))?,
        };
        config.check()?;
        Ok(config)
    }

    fn check(&self) -> Result<()> {
        ensure!(
            self.index.aqua_registry.digest.starts_with("sha256:"),
            "aqua_registry.digest must be sha256:<hex>"
        );
        ensure!(
            (1..=366).contains(&self.index.signing.expires_days),
            "signing.expires_days must be between 1 and 366"
        );
        let mut names = BTreeSet::new();
        for t in &self.launch_set.tools {
            if let Some(n) = &t.name {
                ensure!(
                    !n.contains('/')
                        && crate::index::valid_id(&format!("oyzu:{n}"))
                        && names.insert(n),
                    "launch set: {} has an invalid or repeated name {n:?}",
                    t.package
                );
            }
        }
        ensure!(
            (1..=366).contains(&self.index.verified.retry_after_days),
            "verified.retry_after_days must be between 1 and 366"
        );
        ensure!(
            self.index.verified.keep_records >= self.index.verified.backfill,
            "verified.keep_records must be at least verified.backfill"
        );
        for (name, core) in &self.index.core {
            ensure!(
                core.line.is_none_or(|n| (1..=3).contains(&n)),
                "core.{name}.line must be 1, 2 or 3"
            );
            ensure!(
                core.line.is_some() || (core.lines.is_none() && core.keep_records.is_none()),
                "core.{name}: lines and keep_records need line"
            );
            ensure!(
                core.keep_records
                    .unwrap_or(self.index.verified.keep_records)
                    >= self.index.verified.backfill,
                "core.{name}.keep_records must be at least verified.backfill"
            );
        }
        let mut seen = BTreeSet::new();
        for t in &self.launch_set.tools {
            ensure!(
                seen.insert(&t.package),
                "{} is listed twice in the launch set",
                t.package
            );
        }
        for (id, entry) in &self.serving.tools {
            ensure!(
                id.starts_with("aqua:") || id.starts_with("core:"),
                "serving.toml: {id} is not a tool id"
            );
            if let Some(class) = &entry.class {
                if !["cache", "cache-source-available", "direct", "excluded"]
                    .contains(&class.as_str())
                {
                    bail!("serving.toml: {id} has unknown class {class}");
                }
            }
        }
        Ok(())
    }

    /// Curated short names by package.
    pub fn launch_names(&self) -> BTreeMap<String, String> {
        self.launch_set
            .tools
            .iter()
            .filter_map(|t| Some((t.package.clone(), t.name.clone()?)))
            .collect()
    }

    pub fn launch_packages(&self) -> BTreeSet<String> {
        self.launch_set
            .tools
            .iter()
            .map(|t| t.package.clone())
            .collect()
    }
}

pub fn expand(template: &str, tag: &str) -> String {
    template.replace("{tag}", tag)
}
