//! The job's checked-in configuration: `config/index.toml` (pins and limits),
//! `config/launch-set.toml` (the verified tier) and `config/serving.toml`
//! (the people-maintained license and serving-class supplement).
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

pub struct Config {
    pub index: IndexConfig,
    pub launch_set: LaunchSet,
    pub serving: ServingSupplement,
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
