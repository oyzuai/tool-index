//! Listing upstream versions. `oyzu tools resolve` needs exact versions and
//! never lists them, so the job lists them here and writes a listing file
//! that `build` consumes. Listing is the only network step besides evidence.
//!
//! Sources:
//! - GitHub GraphQL (production): releases with publish dates, or tags for
//!   `version_source: github_tag` packages, batched many repositories per
//!   query so a daily run fits the Actions token's rate limit.
//! - `git ls-remote` (fallback, no token): tags only, no dates.
//! - Adoptium for the Java core provider; the Node.js dist catalog, the Go
//!   download catalog and python-build-standalone's current release for the
//!   Node, Go and Python core providers (the same feeds the CLI reads).
use crate::registry::Package;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub listed_at: String,
    /// Keyed by tool id.
    pub tools: BTreeMap<String, ToolListing>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolListing {
    pub source: String,
    /// Newest first, as the source orders them.
    pub versions: Vec<Listed>,
    /// Set when listing failed; `build` then keeps the previous versions.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Listed {
    pub version: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
}

pub fn aqua_id(package: &str) -> String {
    format!("aqua:{package}")
}

/// Pre-release by SemVer shape, for sources that do not say (tags).
pub fn looks_prerelease(tag: &str) -> bool {
    let t = tag.rsplit('/').next().unwrap_or(tag);
    let t = t.trim_start_matches(|c: char| !c.is_ascii_digit());
    match t.split_once('-') {
        Some((core, pre)) => {
            core.split('.')
                .all(|p| p.chars().all(|c| c.is_ascii_digit()))
                && pre.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Mode {
    Releases,
    Tags,
}

fn mode(p: &Package) -> Mode {
    if p.version_source.as_deref() == Some("github_tag") {
        Mode::Tags
    } else {
        Mode::Releases
    }
}

/// Repositories to list, each with the packages that share it.
fn by_repository(packages: &[Package]) -> BTreeMap<(String, Mode), Vec<String>> {
    let mut out: BTreeMap<(String, Mode), Vec<String>> = BTreeMap::new();
    for p in packages {
        if let Some(repo) = &p.repository {
            out.entry((repo.clone(), mode(p)))
                .or_default()
                .push(p.name.clone());
        }
    }
    out
}

fn graphql_string(s: &str) -> String {
    serde_json::to_string(s).expect("strings serialize")
}

fn batch_query(batch: &[(String, Mode)]) -> String {
    let mut q = String::from("query {\n");
    for (i, (repo, mode)) in batch.iter().enumerate() {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        let body = match mode {
            Mode::Releases => "releases(first: 100, orderBy: {field: CREATED_AT, direction: DESC}) { nodes { tagName publishedAt isPrerelease isDraft } }",
            Mode::Tags => "refs(refPrefix: \"refs/tags/\", first: 100, orderBy: {field: TAG_COMMIT_DATE, direction: DESC}) { nodes { name } }",
        };
        q.push_str(&format!(
            "  r{i}: repository(owner: {}, name: {}) {{ {body} }}\n",
            graphql_string(owner),
            graphql_string(name)
        ));
    }
    q.push_str("  rateLimit { remaining resetAt }\n}\n");
    q
}

fn parse_batch_node(node: &Value, mode: Mode) -> Vec<Listed> {
    match mode {
        Mode::Releases => node["releases"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| !r["isDraft"].as_bool().unwrap_or(false))
            .filter_map(|r| {
                Some(Listed {
                    version: r["tagName"].as_str()?.to_owned(),
                    published_at: r["publishedAt"].as_str().map(str::to_owned),
                    prerelease: r["isPrerelease"].as_bool().unwrap_or(false),
                })
            })
            .collect(),
        Mode::Tags => node["refs"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["name"].as_str())
            .map(|t| Listed {
                version: t.to_owned(),
                published_at: None,
                prerelease: looks_prerelease(t),
            })
            .collect(),
    }
}

/// Lists through the GitHub GraphQL API, `batch` repositories per query.
pub async fn list_github(
    packages: &[Package],
    endpoint: &str,
    token: &str,
    batch: usize,
) -> Result<BTreeMap<String, ToolListing>> {
    let client = reqwest::Client::builder()
        .user_agent("oyzu-tool-index")
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let repos = by_repository(packages);
    let keys: Vec<(String, Mode)> = repos.keys().cloned().collect();
    let mut out = BTreeMap::new();
    for chunk in keys.chunks(batch.max(1)) {
        let response = client
            .post(endpoint)
            .bearer_auth(token)
            .json(&json!({ "query": batch_query(chunk) }))
            .send()
            .await
            .context("GitHub GraphQL request")?;
        let status = response.status();
        let body: Value = response.json().await.context("GitHub GraphQL response")?;
        if !status.is_success() {
            bail!("GitHub GraphQL returned HTTP {status}: {body}");
        }
        // Per-repository errors (renamed or deleted repositories) arrive in
        // `errors` with a path; the rest of the batch still has data.
        let mut failed: BTreeMap<String, String> = BTreeMap::new();
        for e in body["errors"].as_array().into_iter().flatten() {
            if let Some(alias) = e["path"][0].as_str() {
                failed.insert(
                    alias.to_owned(),
                    e["message"].as_str().unwrap_or("error").to_owned(),
                );
            }
        }
        for (i, key) in chunk.iter().enumerate() {
            let alias = format!("r{i}");
            let node = &body["data"][&alias];
            let listing = if node.is_null() {
                ToolListing {
                    source: source_name(key.1).into(),
                    versions: Vec::new(),
                    error: Some(
                        failed
                            .get(&alias)
                            .cloned()
                            .unwrap_or_else(|| "not found".into()),
                    ),
                }
            } else {
                ToolListing {
                    source: source_name(key.1).into(),
                    versions: parse_batch_node(node, key.1),
                    error: None,
                }
            };
            for package in &repos[key] {
                out.insert(aqua_id(package), listing.clone());
            }
        }
    }
    Ok(out)
}

fn source_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Releases => "github-releases",
        Mode::Tags => "github-tags",
    }
}

/// Whether a GraphQL error is an owner's IP allow list refusing the token.
/// Those repositories are listed again with `git ls-remote`.
pub fn is_ip_allow_list(message: &str) -> bool {
    message.contains("IP allow list")
}

/// Parses `git ls-remote --tags --refs` output.
pub fn parse_ls_remote(output: &str) -> Vec<Listed> {
    let mut tags: Vec<Listed> = output
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(_, r)| r.strip_prefix("refs/tags/"))
        .map(|t| Listed {
            version: t.to_owned(),
            published_at: None,
            prerelease: looks_prerelease(t),
        })
        .collect();
    // No dates: order by version, newest first, so `max_versions` keeps the
    // newest. Tags that are not versions sort last.
    tags.sort_by_key(|t| std::cmp::Reverse(sort_key(&t.version)));
    tags
}

pub fn sort_key(tag: &str) -> (bool, Vec<u64>, String) {
    let t = tag.rsplit('/').next().unwrap_or(tag);
    let digits = t.trim_start_matches(|c: char| !c.is_ascii_digit());
    let core = digits.split(['-', '+']).next().unwrap_or("");
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    match parts {
        // A release sorts above its pre-releases: `true` for no suffix.
        Some(p) if !p.is_empty() => (
            true,
            p,
            if digits.len() == core.len() {
                "~".into()
            } else {
                digits[core.len()..].to_owned()
            },
        ),
        _ => (false, Vec::new(), tag.to_owned()),
    }
}

/// Lists tags with `git ls-remote`, which needs no token and no API access.
pub async fn list_git(
    packages: &[Package],
    parallel: usize,
) -> Result<BTreeMap<String, ToolListing>> {
    let repos = by_repository(packages);
    let sem = Arc::new(Semaphore::new(parallel.max(1)));
    let mut tasks = Vec::new();
    for ((repo, _), names) in repos {
        let sem = sem.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore open");
            let result = tokio::process::Command::new("git")
                .args([
                    "ls-remote",
                    "--tags",
                    "--refs",
                    &format!("https://github.com/{repo}"),
                ])
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .await;
            let listing = match result {
                Ok(o) if o.status.success() => ToolListing {
                    source: "git-tags".into(),
                    versions: parse_ls_remote(&String::from_utf8_lossy(&o.stdout)),
                    error: None,
                },
                Ok(o) => ToolListing {
                    source: "git-tags".into(),
                    versions: Vec::new(),
                    error: Some(String::from_utf8_lossy(&o.stderr).trim().to_owned()),
                },
                Err(e) => ToolListing {
                    source: "git-tags".into(),
                    versions: Vec::new(),
                    error: Some(e.to_string()),
                },
            };
            (names, listing)
        }));
    }
    let mut out = BTreeMap::new();
    for t in tasks {
        let (names, listing) = t.await?;
        for n in names {
            out.insert(aqua_id(&n), listing.clone());
        }
    }
    Ok(out)
}

/// Java (Eclipse Temurin) GA releases from the Adoptium API.
pub async fn list_adoptium(base: &str, max: usize) -> Result<ToolListing> {
    let client = reqwest::Client::builder()
        .user_agent("oyzu-tool-index")
        .build()?;
    let url = format!(
        "{base}/v3/info/release_versions?release_type=ga&vendor=eclipse&image_type=jdk&page=0&page_size={}&sort_order=DESC",
        max.clamp(1, 50)
    );
    let body: Value = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(parse_adoptium(&body))
}

pub fn parse_adoptium(body: &Value) -> ToolListing {
    let mut versions: Vec<Listed> = Vec::new();
    for v in body["versions"].as_array().into_iter().flatten() {
        let (Some(major), Some(minor), Some(security)) = (
            v["major"].as_u64(),
            v["minor"].as_u64(),
            v["security"].as_u64(),
        ) else {
            continue;
        };
        let version = format!("{major}.{minor}.{security}");
        if !versions.iter().any(|l| l.version == version) {
            versions.push(Listed {
                version,
                published_at: None,
                prerelease: false,
            });
        }
    }
    ToolListing {
        source: "adoptium".into(),
        versions,
        error: None,
    }
}

async fn get(url: &str, max: usize) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent("oyzu-tool-index")
        .build()?;
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    anyhow::ensure!(body.len() <= max, "{url} is larger than {max} bytes");
    Ok(body.to_vec())
}

/// A `MAJOR.MINOR.PATCH` release of digits only: the core providers record
/// exact stable releases.
fn exact_release(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn listing(source: &str, versions: Vec<Listed>) -> ToolListing {
    ToolListing {
        source: source.into(),
        versions,
        error: None,
    }
}

pub const NODE_CATALOG: &str = "https://nodejs.org/dist/index.json";
pub const GO_CATALOG: &str = "https://go.dev/dl/?mode=json&include=all";
pub const PYTHON_RELEASE: &str = "https://raw.githubusercontent.com/astral-sh/python-build-standalone/latest-release/latest-release.json";

/// Node.js releases from the dist catalog, with their release days.
pub async fn list_node(url: &str) -> Result<ToolListing> {
    let body: Value = serde_json::from_slice(&get(url, 16 << 20).await?)?;
    Ok(parse_node(&body))
}

pub fn parse_node(body: &Value) -> ToolListing {
    let versions = body
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let version = r["version"].as_str()?.strip_prefix('v')?;
            exact_release(version).then(|| Listed {
                version: version.to_owned(),
                published_at: r["date"]
                    .as_str()
                    .filter(|d| d.parse::<jiff::civil::Date>().is_ok())
                    .map(|d| format!("{d}T00:00:00Z")),
                prerelease: false,
            })
        })
        .collect();
    listing("nodejs.org", versions)
}

/// Stable Go releases from the full download catalog (`go1.25.1`; releases
/// without a patch number, such as `go1.20`, are not installable versions).
pub async fn list_go(url: &str) -> Result<ToolListing> {
    let body: Value = serde_json::from_slice(&get(url, 16 << 20).await?)?;
    Ok(parse_go(&body))
}

pub fn parse_go(body: &Value) -> ToolListing {
    let versions = body
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["stable"].as_bool() == Some(true))
        .filter_map(|r| {
            let version = r["version"].as_str()?.strip_prefix("go")?;
            exact_release(version).then(|| Listed {
                version: version.to_owned(),
                published_at: None,
                prerelease: false,
            })
        })
        .collect();
    listing("go.dev", versions)
}

/// The python-build-standalone release triples the CLI installs; a version
/// is listed only when the release ships it for all of them.
const PYTHON_TRIPLES: [&str; 3] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

/// CPython versions in python-build-standalone's current release: the
/// release pointer names a tag, whose `SHA256SUMS` lists the distributions.
pub async fn list_python(pointer: &str) -> Result<ToolListing> {
    let index: Value = serde_json::from_slice(&get(pointer, 64 << 10).await?)?;
    let tag = index["tag"]
        .as_str()
        .filter(|t| t.len() == 8 && t.bytes().all(|b| b.is_ascii_digit()))
        .context("the python-build-standalone release pointer names no tag")?;
    let prefix = index["asset_url_prefix"]
        .as_str()
        .context("the python-build-standalone release pointer names no asset location")?;
    let sums = get(&format!("{prefix}/SHA256SUMS"), 8 << 20).await?;
    Ok(parse_python(tag, &String::from_utf8_lossy(&sums)))
}

pub fn parse_python(tag: &str, sums: &str) -> ToolListing {
    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    for name in sums.lines().filter_map(|l| l.split_whitespace().nth(1)) {
        let Some(rest) = name
            .strip_prefix("cpython-")
            .and_then(|r| r.strip_suffix("-install_only.tar.gz"))
        else {
            continue;
        };
        let Some((version, triple)) = rest.split_once('+').and_then(|(v, r)| {
            r.strip_prefix(tag)
                .and_then(|r| r.strip_prefix('-'))
                .map(|t| (v, t))
        }) else {
            continue;
        };
        if exact_release(version) && PYTHON_TRIPLES.contains(&triple) {
            *found.entry(version.to_owned()).or_default() += 1;
        }
    }
    let mut versions: Vec<Listed> = found
        .into_iter()
        .filter(|(_, n)| *n == PYTHON_TRIPLES.len())
        .map(|(version, _)| Listed {
            version,
            published_at: None,
            prerelease: false,
        })
        .collect();
    versions.sort_by_key(|l| std::cmp::Reverse(sort_key(&l.version)));
    listing("python-build-standalone", versions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prerelease_shapes() {
        assert!(looks_prerelease("v1.2.3-rc.1"));
        assert!(looks_prerelease("cli/v2.0.0-beta"));
        assert!(!looks_prerelease("v1.2.3"));
        assert!(!looks_prerelease("2024-01-01"));
    }

    #[test]
    fn ip_allow_list_errors_are_recognized() {
        assert!(is_ip_allow_list("Although you appear to have the correct authorization credentials, the `Shopify` organization has an IP allow list enabled, and your IP address is not permitted to access this resource."));
        assert!(!is_ip_allow_list(
            "Could not resolve to a Repository with the name 'RoseSecurity/Kuzco'."
        ));
    }

    #[test]
    fn ls_remote_sorts_newest_first() {
        let out = "a\trefs/tags/v1.9.0\nb\trefs/tags/v1.10.0\nc\trefs/tags/v1.10.0-rc.1\nd\trefs/tags/latest\n";
        let v: Vec<_> = parse_ls_remote(out)
            .into_iter()
            .map(|l| l.version)
            .collect();
        assert_eq!(v, ["v1.10.0", "v1.10.0-rc.1", "v1.9.0", "latest"]);
    }

    #[test]
    fn graphql_batch_groups_shared_repositories() {
        let p = |name: &str, repo: &str| Package {
            name: name.into(),
            source_kind: "http".into(),
            repository: Some(repo.into()),
            version_source: None,
        };
        let packages = [
            p("kubernetes/kubernetes/kubectl", "kubernetes/kubernetes"),
            p("kubernetes/kubernetes/kubeadm", "kubernetes/kubernetes"),
        ];
        let repos = by_repository(&packages);
        assert_eq!(repos.len(), 1);
        let q = batch_query(&repos.keys().cloned().collect::<Vec<_>>());
        assert!(q.contains("r0: repository(owner: \"kubernetes\", name: \"kubernetes\")"));
        let node = json!({"releases": {"nodes": [
            {"tagName": "v1.31.0", "publishedAt": "2024-08-13T00:00:00Z", "isPrerelease": false, "isDraft": false},
            {"tagName": "v1.32.0", "publishedAt": null, "isPrerelease": false, "isDraft": true}
        ]}});
        let listed = parse_batch_node(&node, Mode::Releases);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].published_at.as_deref(),
            Some("2024-08-13T00:00:00Z")
        );
    }

    #[test]
    fn adoptium_versions_drop_the_build() {
        let body = json!({"versions": [
            {"major": 21, "minor": 0, "security": 8, "build": 9},
            {"major": 21, "minor": 0, "security": 8, "build": 7},
            {"major": 17, "minor": 0, "security": 16, "build": 8}
        ]});
        let l = parse_adoptium(&body);
        assert_eq!(
            l.versions
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["21.0.8", "17.0.16"]
        );
    }

    #[test]
    fn node_go_and_python_list_exact_stable_releases() {
        let node = parse_node(&json!([
            {"version": "v24.9.0", "date": "2025-09-25"},
            {"version": "v22.20.0", "date": "2025-09-24"},
            {"version": "v0.1.14", "date": "2011-08-26"},
            {"version": "nightly"}
        ]));
        assert_eq!(node.versions.len(), 3);
        assert_eq!(node.versions[1].version, "22.20.0");
        assert_eq!(
            node.versions[1].published_at.as_deref(),
            Some("2025-09-24T00:00:00Z")
        );

        let go = parse_go(&json!([
            {"version": "go1.26rc1", "stable": false},
            {"version": "go1.25.1", "stable": true},
            {"version": "go1.20", "stable": true}
        ]));
        let names: Vec<_> = go.versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(names, ["1.25.1"]);

        let tag = "20250918";
        let mut sums = String::new();
        for triple in PYTHON_TRIPLES {
            for version in ["3.13.7", "3.9.23"] {
                sums.push_str(&format!(
                    "{}  cpython-{version}+{tag}-{triple}-install_only.tar.gz\n",
                    "a".repeat(64)
                ));
            }
        }
        sums.push_str(&format!(
            "{}  cpython-3.14.0+{tag}-x86_64-unknown-linux-gnu-install_only.tar.gz\n",
            "b".repeat(64)
        ));
        sums.push_str(&format!(
            "{}  cpython-3.13.7+{tag}-x86_64-unknown-linux-gnu-debug-full.tar.zst\n",
            "c".repeat(64)
        ));
        let python = parse_python(tag, &sums);
        let names: Vec<_> = python.versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(names, ["3.13.7", "3.9.23"], "3.14.0 lacks two platforms");
    }
}
