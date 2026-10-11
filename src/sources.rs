//! Source archives for copyleft tools served from Oyzu's host.
//!
//! GPL, LGPL and AGPL binaries may be served only with their source: under
//! GPLv2 a commercial distributor cannot point at upstream, and under GPLv3 it
//! stays responsible if upstream's copy goes away. So for a tool whose
//! reviewed serving class is `cache` with a copyleft license
//! (`Tool::source_mirror`), the job finds the source archive of every version
//! with records, downloads it once, and records its URL, SHA-256 and size in
//! the index. A delivery service mirrors those bytes beside the binary, and
//! serves a version without a `source` directly from upstream instead.
//!
//! Nothing is mirrored until a person reviews a copyleft tool in
//! `config/serving.toml`; until then no tool has `source_mirror`.
use crate::config::Config;
use crate::index::{Index, Source};
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};

/// Largest source archive the job downloads (OpenJDK's is about 110 MB).
pub const MAX_SOURCE_BYTES: u64 = 2 << 30;

/// Where a tool's source archive comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Locator {
    /// GitHub's archive of the release tag:
    /// `https://github.com/<repository>/archive/refs/tags/<tag>.tar.gz`.
    GithubArchive,
    /// The `jdk-sources` package Adoptium attaches to the GitHub release
    /// that holds the record's artifact (Temurin).
    Adoptium,
    /// A URL with `{tag}`, `{version}` and `{repository}` replaced.
    Template(String),
}

impl Locator {
    /// `serving.toml`'s `source` for the tool, else the default for its origin:
    /// GitHub's tag archive for an aqua package, Adoptium for `core:java`.
    pub fn for_tool(origin: &str, configured: Option<&str>) -> Option<Self> {
        match configured {
            Some("github-archive") => Some(Self::GithubArchive),
            Some("adoptium") => Some(Self::Adoptium),
            Some(t) => Some(Self::Template(t.to_owned())),
            None if origin.starts_with("aqua:") => Some(Self::GithubArchive),
            None if origin == "core:java" => Some(Self::Adoptium),
            None => None,
        }
    }
}

/// One version that needs its source archive found and hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub tool: String,
    pub version: String,
    /// The upstream tag (the records' `version.requested`).
    pub tag: String,
    /// The records' `tool.repository`, when the source is GitHub.
    pub repository: Option<String>,
    /// The resolved record's artifact URL.
    pub artifact: Option<String>,
    pub locator: Locator,
}

/// Versions of `source_mirror` tools that have records but no source yet.
pub fn wanted(index: &Index, config: &Config) -> Vec<Want> {
    let mut out = Vec::new();
    for tool in index.tools.iter().filter(|t| t.source_mirror) {
        let configured = config
            .serving
            .tools
            .get(&tool.origin)
            .and_then(|e| e.source.as_deref());
        let Some(locator) = Locator::for_tool(&tool.origin, configured) else {
            continue;
        };
        for v in &tool.versions {
            let Some(record) = v.sets.iter().find(|r| r["status"] == "resolved") else {
                continue;
            };
            if v.source.is_some() {
                continue;
            }
            out.push(Want {
                tool: tool.id.clone(),
                version: v.version.clone(),
                tag: record["version"]["requested"]
                    .as_str()
                    .unwrap_or(&v.version)
                    .to_owned(),
                repository: record["tool"]["repository"].as_str().map(str::to_owned),
                artifact: record["urls"].as_array().and_then(|urls| {
                    urls.iter()
                        .find(|u| u["role"] == "artifact")
                        .and_then(|u| u["url"].as_str())
                        .map(str::to_owned)
                }),
                locator: locator.clone(),
            });
        }
    }
    out
}

/// The archive URL for a want.
pub fn locate(want: &Want) -> Result<String> {
    let url = match &want.locator {
        Locator::GithubArchive => {
            let Some(repo) = &want.repository else {
                bail!("the records name no GitHub repository");
            };
            format!(
                "https://github.com/{repo}/archive/refs/tags/{}.tar.gz",
                want.tag
            )
        }
        Locator::Template(t) => t
            .replace("{tag}", &want.tag)
            .replace("{version}", &want.version)
            .replace(
                "{repository}",
                want.repository.as_deref().unwrap_or_default(),
            ),
        Locator::Adoptium => {
            let Some(artifact) = &want.artifact else {
                bail!("the records name no artifact");
            };
            temurin_source(artifact)?
        }
    };
    ensure!(url.starts_with("https://"), "source URL {url} is not https");
    Ok(url)
}

/// The source package beside a Temurin JDK artifact: in the same GitHub
/// release, `OpenJDK21U-jdk_x64_linux_hotspot_21.0.8_9.tar.gz` has
/// `OpenJDK21U-jdk-sources_21.0.8_9.tar.gz`. Adoptium's assets API is not
/// used: it matches only full versions with a build number, so a plain
/// `21.0.8` finds nothing.
pub fn temurin_source(artifact: &str) -> Result<String> {
    let (dir, file) = artifact
        .rsplit_once('/')
        .context("the artifact URL has no file name")?;
    let parsed = (|| {
        let (major, rest) = file.split_once("-jdk_")?;
        // <arch>_<os>_hotspot_<version>_<build>.<ext>
        let (_, rest) = rest.split_once("_hotspot_")?;
        let stem = rest
            .strip_suffix(".tar.gz")
            .or_else(|| rest.strip_suffix(".zip"))?;
        Some(format!("{dir}/{major}-jdk-sources_{stem}.tar.gz"))
    })();
    ensure!(
        dir.starts_with("https://github.com/adoptium/") && parsed.is_some(),
        "{artifact} is not a Temurin JDK release asset"
    );
    Ok(parsed.unwrap())
}

/// Downloads `url` and returns its digest and size, refusing anything over
/// [`MAX_SOURCE_BYTES`].
pub async fn fetch(client: &reqwest::Client, url: &str) -> Result<Source> {
    let mut resp = client.get(url).send().await?.error_for_status()?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    while let Some(chunk) = resp.chunk().await? {
        size += chunk.len() as u64;
        ensure!(
            size <= MAX_SOURCE_BYTES,
            "{url} is larger than {MAX_SOURCE_BYTES} bytes"
        );
        hasher.update(&chunk);
    }
    Ok(Source {
        url: url.to_owned(),
        sha256: format!("sha256:{}", hex::encode(hasher.finalize())),
        size,
    })
}

/// Records found sources in the index. Returns how many were applied.
pub fn apply(index: &mut Index, found: &[(Want, Source)]) -> usize {
    let mut n = 0;
    for (want, source) in found {
        let Some(tool) = index.tools.iter_mut().find(|t| t.id == want.tool) else {
            continue;
        };
        if let Some(v) = tool.versions.iter_mut().find(|v| v.version == want.version) {
            v.source = Some(source.clone());
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locators_default_by_origin() {
        assert_eq!(
            Locator::for_tool("aqua:cli/cli", None),
            Some(Locator::GithubArchive)
        );
        assert_eq!(
            Locator::for_tool("core:java", None),
            Some(Locator::Adoptium)
        );
        assert_eq!(Locator::for_tool("core:node", None), None);
        assert_eq!(
            Locator::for_tool("core:node", Some("https://x/{version}.tar.gz")),
            Some(Locator::Template("https://x/{version}.tar.gz".into()))
        );
    }

    #[test]
    fn temurin_source_sits_beside_the_jdk() {
        assert_eq!(
            temurin_source("https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1+1/OpenJDK21U-jdk_aarch64_mac_hotspot_21.0.12.1_1.tar.gz").unwrap(),
            "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1+1/OpenJDK21U-jdk-sources_21.0.12.1_1.tar.gz"
        );
        assert_eq!(
            temurin_source("https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25+36/OpenJDK25U-jdk_x64_windows_hotspot_25_36.zip").unwrap(),
            "https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25+36/OpenJDK25U-jdk-sources_25_36.tar.gz"
        );
        assert!(temurin_source(
            "https://github.com/adoptium/temurin21-binaries/releases/download/x/jdk.tar.gz"
        )
        .is_err());
        assert!(
            temurin_source("https://example.com/OpenJDK21U-jdk_x64_linux_hotspot_21_9.tar.gz")
                .is_err()
        );
    }
}
