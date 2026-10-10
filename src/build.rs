//! Assembling `index.json` from the pinned snapshot, the version listing, the
//! previous index and, for the verified tier, `oyzu tools resolve` and
//! `oyzu tools evidence`.
//!
//! The previous index is the job's only state: it carries first-seen dates
//! and already-verified records forward, so a record is downloaded and checked
//! once, when its version first appears.
use crate::config::{Config, CoreProvider};
use crate::index::{self, DateSource, Index, Released, ServingClass, Snapshot, Tool, Version};
use crate::registry;
use crate::resolve;
use crate::serving::serving_for_tool;
use crate::versions::{aqua_id, Listed, Listing};
use anyhow::{Context, Result};
use jiff::Timestamp;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub struct Inputs<'a> {
    pub config: &'a Config,
    pub registry_path: PathBuf,
    pub listing: Listing,
    pub previous: Option<Index>,
    pub now: Timestamp,
    /// Runs `oyzu`; `None` skips the verified tier and carries records forward.
    pub oyzu: Option<resolve::Oyzu>,
    /// Restrict the run to these origins (`aqua:<package>`, `core:<provider>`).
    pub only: Option<BTreeSet<String>>,
    pub work: PathBuf,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub tools: usize,
    pub versions: usize,
    pub verified_records: usize,
    pub verified_this_run: usize,
    pub verification_failed: Vec<String>,
    pub deferred: usize,
    pub listing_errors: Vec<String>,
    pub excluded: Vec<String>,
    pub id_conflicts: Vec<String>,
}

const FILTER_PLATFORM: &str = "linux/amd64/gnu";

/// Why a version's records did not reach the verified tier: the first failed
/// check of the first record that has one, as `oyzu tools evidence` gave it.
fn failure_reason(records: &[Value]) -> String {
    records
        .iter()
        .find_map(|r| {
            r["verified"]["results"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["outcome"] == "failed")
                .map(|c| {
                    format!(
                        "{} {}: {}",
                        r["platform"].as_str().unwrap_or("?"),
                        c["evidence"].as_str().unwrap_or("?"),
                        c["detail"].as_str().unwrap_or("failed")
                    )
                })
        })
        .unwrap_or_else(|| "no record reached the verified tier".into())
}
/// Engine version recorded when no resolve ran and there is no previous index.
/// The evaluator digest, which identifies the rules, is always computed.
const ENGINE_FALLBACK_VERSION: &str = "0.1.0";

/// Newest first by version shape, then by date. Version order is used because
/// tag sources give no dates, and first-seen dates say nothing about order.
fn order(a: &Version, b: &Version) -> std::cmp::Ordering {
    crate::versions::sort_key(&b.version)
        .cmp(&crate::versions::sort_key(&a.version))
        .then_with(|| b.released.time.cmp(&a.released.time))
}

/// Normalized version for each listed tag the recipe covers, using the same
/// engine as the CLI: `version_prefix` and `version_filter` drop tags of other
/// packages in a shared repository and non-release tags.
fn normalize(
    snapshot: &oyzu_recipe::Snapshot,
    package: &str,
    tags: &[Listed],
) -> BTreeMap<String, String> {
    if tags.is_empty() {
        return BTreeMap::new();
    }
    let request = oyzu_recipe::Request {
        tools: vec![package.to_owned()],
        versions: tags.iter().map(|t| t.version.clone()).collect(),
        platforms: vec![FILTER_PLATFORM.to_owned()],
        limits: oyzu_recipe::Limits::default(),
    };
    match oyzu_recipe::evaluate(snapshot, &request) {
        Ok(records) => records
            .into_iter()
            .filter(|r| {
                r.reason
                    .as_ref()
                    .is_none_or(|r| r.code != "VERSION_NOT_COVERED")
            })
            .map(|r| (r.version.requested, r.version.normalized))
            .filter(|(_, n)| looks_like_version(n))
            .collect(),
        // The recipe cannot be evaluated at all: list what the publisher
        // tagged, normalized the plain way. A standalone client gets the same
        // error from local evaluation.
        Err(_) => tags
            .iter()
            .map(|t| {
                (
                    t.version.clone(),
                    t.version.trim_start_matches('v').to_owned(),
                )
            })
            .filter(|(_, n)| looks_like_version(n))
            .collect(),
    }
}

/// Repositories tag more than releases (`nightly-…`, `pkg/v3.1.0` for an SDK,
/// `chart-3.9.1`). A recipe without a `version_filter` lets them through, so
/// a version must start with a number (`1.2.3`, `2025-08-13T08-35-41Z`).
fn looks_like_version(normalized: &str) -> bool {
    normalized
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_digit())
        && !normalized.contains('/')
        && !normalized.contains('@')
}

/// Merges listed versions with the previous entry. A known version keeps its
/// date and records; a new one is dated by the publisher or first seen now.
fn merge(listed: &[(String, &Listed)], previous: Option<&Tool>, now: &str) -> Vec<Version> {
    let mut out: BTreeMap<String, Version> = BTreeMap::new();
    if let Some(prev) = previous {
        for v in &prev.versions {
            out.insert(v.version.clone(), v.clone());
        }
    }
    for (normalized, l) in listed {
        out.entry(normalized.clone()).or_insert_with(|| Version {
            version: normalized.clone(),
            released: Released {
                time: l.published_at.clone().unwrap_or_else(|| now.to_owned()),
                source: if l.published_at.is_some() {
                    DateSource::Publisher
                } else {
                    DateSource::FirstSeen
                },
            },
            sets: Vec::new(),
        });
    }
    let mut v: Vec<Version> = out.into_values().collect();
    v.sort_by(order);
    v
}

/// The canonical id for a package, as the oyzu CLI spells it in the lock:
/// `oyzu:<name>` for a launch-set tool with a curated name, otherwise
/// `oyzu:<package>` lowercased.
pub fn canonical_id(package: &str, curated: Option<&str>) -> String {
    match curated {
        Some(name) => format!("oyzu:{name}"),
        None => format!("oyzu:{}", package.to_ascii_lowercase()),
    }
}

fn listing_for<'a>(listing: &'a Listing, origin: &str, report: &mut Report) -> &'a [Listed] {
    match listing.tools.get(origin) {
        Some(l) if l.error.is_none() => &l.versions,
        Some(l) => {
            report
                .listing_errors
                .push(format!("{origin}: {}", l.error.as_deref().unwrap_or("")));
            &[]
        }
        None => &[],
    }
}

struct Pending {
    tool: usize,
    source: PendingSource,
    /// Upstream tags to resolve, which `tools resolve` takes as `--version`;
    /// for a core provider, its exact versions.
    tags: Vec<String>,
}

/// Where a pending tool's records come from.
enum PendingSource {
    /// A registry package: `oyzu tools resolve`, then `oyzu tools evidence`.
    Package(String),
    /// A core provider: `oyzu tools evidence --provider <name>`.
    Provider(String),
}

/// Versions to verify this run: on a tool's first run only the newest
/// `backfill`, afterwards every version the previous index did not list.
/// Returns the candidates' normalized versions, newest first.
fn candidates(
    versions: &[Version],
    previous: Option<&Tool>,
    cfg: &crate::config::Verified,
) -> Vec<String> {
    let first_run = previous.is_none_or(|p| p.versions.iter().all(|v| v.sets.is_empty()));
    let known: BTreeSet<&str> = previous
        .into_iter()
        .flat_map(|p| &p.versions)
        .map(|v| v.version.as_str())
        .collect();
    versions
        .iter()
        .filter(|v| cfg.include_prereleases || !index::prerelease(&v.version))
        .enumerate()
        .filter(|(i, v)| {
            !index::verified(v)
                && (*i < cfg.backfill || (!first_run && !known.contains(v.version.as_str())))
        })
        .map(|(_, v)| v.version.clone())
        .collect()
}

/// A core provider's versions, newest first, cut to `max_versions`: overall,
/// or per release line for the newest `lines` lines when it counts by line.
fn keep_lines(versions: &mut Vec<Version>, core: &CoreProvider) {
    if core.line.is_none() {
        versions.truncate(core.max_versions);
        return;
    }
    let mut lines: Vec<String> = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    versions.retain(|v| {
        let line = core.line_of(&v.version).unwrap_or("").to_owned();
        if !lines.contains(&line) {
            lines.push(line.clone());
        }
        let n = counts.entry(line.clone()).or_default();
        *n += 1;
        core.lines
            .is_none_or(|max| lines.iter().position(|l| *l == line).unwrap_or(0) < max)
            && *n <= core.max_versions
    });
}

/// [`candidates`] for a core provider: per release line when it counts by
/// line, so a line without records gets its backfill even when newer lines
/// have records. Lines take turns, so a short budget reaches every line.
fn core_candidates(
    versions: &[Version],
    previous: Option<&Tool>,
    core: &CoreProvider,
    cfg: &crate::config::Verified,
) -> Vec<String> {
    if core.line.is_none() {
        return candidates(versions, previous, cfg);
    }
    let mut lines: Vec<(&str, Vec<Version>)> = Vec::new();
    for v in versions {
        let line = core.line_of(&v.version).unwrap_or("");
        match lines.iter_mut().find(|(l, _)| *l == line) {
            Some((_, group)) => group.push(v.clone()),
            None => lines.push((line, vec![v.clone()])),
        }
    }
    let per_line: Vec<Vec<String>> = lines
        .iter()
        .map(|(line, group)| {
            let previous = previous.map(|p| Tool {
                versions: p
                    .versions
                    .iter()
                    .filter(|v| core.line_of(&v.version) == Some(line))
                    .cloned()
                    .collect(),
                ..p.clone()
            });
            candidates(group, previous.as_ref(), cfg)
        })
        .collect();
    let longest = per_line.iter().map(Vec::len).max().unwrap_or(0);
    (0..longest)
        .flat_map(|i| per_line.iter().filter_map(move |c| c.get(i).cloned()))
        .collect()
}

pub fn build(inputs: Inputs<'_>) -> Result<(Index, Report)> {
    let cfg = &inputs.config.index;
    let registry_bytes = std::fs::read(&inputs.registry_path)
        .with_context(|| format!("reading {}", inputs.registry_path.display()))?;
    registry::check_digest(&registry_bytes, &cfg.aqua_registry.digest)?;
    let snapshot = oyzu_recipe::Snapshot::parse(&registry_bytes, &oyzu_recipe::Limits::default())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let curated = inputs.config.launch_names();
    let launch = inputs.config.launch_packages();
    let now = inputs.now.to_string();
    let wanted = |origin: &str| inputs.only.as_ref().is_none_or(|o| o.contains(origin));
    let previous = |origin: &str| inputs.previous.as_ref().and_then(|p| p.by_origin(origin));

    let mut report = Report::default();
    let mut tools: Vec<Tool> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut pending: Vec<Pending> = Vec::new();
    let mut budget = cfg.verified.max_versions_per_run;

    // Core providers first, so they own their short names (`java`).
    for (provider, core) in &cfg.core {
        let origin = format!("core:{provider}");
        if !wanted(&origin) {
            continue;
        }
        let serving = serving_for_tool(&origin, inputs.config.serving.tools.get(&origin));
        if serving.class == ServingClass::Excluded {
            report.excluded.push(origin);
            continue;
        }
        let listed: Vec<(String, &Listed)> = listing_for(&inputs.listing, &origin, &mut report)
            .iter()
            .map(|l| (l.version.clone(), l))
            .collect();
        let mut versions = merge(&listed, previous(&origin), &now);
        keep_lines(&mut versions, core);
        let id = format!(
            "oyzu:{}",
            core.names.first().map(String::as_str).unwrap_or(provider)
        );
        ids.insert(id.clone());
        if core.verified {
            // Records come from `oyzu tools evidence --provider`, with the
            // record id `core:<provider>` the index carries as the origin.
            let wanted = core_candidates(&versions, previous(&origin), core, &cfg.verified);
            let take = wanted.len().min(budget);
            report.deferred += wanted.len() - take;
            budget -= take;
            if take > 0 {
                pending.push(Pending {
                    tool: tools.len(),
                    source: PendingSource::Provider(provider.clone()),
                    tags: wanted[..take].to_vec(),
                });
            }
        } else {
            for v in &mut versions {
                v.sets.clear();
            }
        }
        tools.push(Tool {
            id,
            origin,
            serving_class: serving.class,
            versions,
        });
    }

    for package in registry::packages(&registry_bytes)? {
        let origin = aqua_id(&package.name);
        let is_launch = launch.contains(&package.name);
        if !wanted(&origin)
            || (!is_launch && !cfg.discovery.source_kinds.contains(&package.source_kind))
        {
            continue;
        }
        let serving = serving_for_tool(&origin, inputs.config.serving.tools.get(&origin));
        if serving.class == ServingClass::Excluded {
            report.excluded.push(origin);
            continue;
        }
        let mut id = canonical_id(
            &package.name,
            curated.get(&package.name).map(String::as_str),
        );
        if ids.contains(&id) {
            let long = format!("oyzu:{}", package.name.to_ascii_lowercase());
            report
                .id_conflicts
                .push(format!("{origin}: {id} is taken, using {long}"));
            id = long;
        }
        if !index::valid_id(&id) || !ids.insert(id.clone()) {
            report
                .id_conflicts
                .push(format!("{origin}: no usable id ({id}); left out"));
            continue;
        }
        let prev = previous(&origin);
        let tags = listing_for(&inputs.listing, &origin, &mut report);
        let normalized = normalize(&snapshot, &package.name, tags);
        // The newest tag wins when two tags normalize alike (`v1.0`, `1.0`).
        let mut tag_of: BTreeMap<String, String> = BTreeMap::new();
        let mut listed: Vec<(String, &Listed)> = Vec::new();
        for l in tags {
            if let Some(n) = normalized.get(&l.version) {
                if !tag_of.contains_key(n) {
                    tag_of.insert(n.clone(), l.version.clone());
                    listed.push((n.clone(), l));
                }
            }
        }
        let mut versions = merge(&listed, prev, &now);
        versions.truncate(cfg.discovery.max_versions);
        if !is_launch {
            for v in &mut versions {
                v.sets.clear();
            }
        } else {
            // Verify versions with no verified records yet: on a tool's first
            // run only the newest few, afterwards every new version.
            let wanted: Vec<String> = candidates(&versions, prev, &cfg.verified)
                .iter()
                .filter_map(|v| tag_of.get(v).cloned())
                .collect();
            let take = wanted.len().min(budget);
            report.deferred += wanted.len() - take;
            budget -= take;
            if take > 0 {
                pending.push(Pending {
                    tool: tools.len(),
                    source: PendingSource::Package(package.name.clone()),
                    tags: wanted[..take].to_vec(),
                });
            }
        }
        tools.push(Tool {
            id,
            origin,
            serving_class: serving.class,
            versions,
        });
    }

    let mut engine: Option<Value> = None;
    if let Some(oyzu) = &inputs.oyzu {
        std::fs::create_dir_all(&inputs.work)?;
        for p in &pending {
            let tool = &mut tools[p.tool];
            let result = match &p.source {
                PendingSource::Package(package) => resolve::resolve_and_verify(
                    oyzu,
                    &inputs.registry_path,
                    &cfg.aqua_registry,
                    package,
                    &p.tags,
                    &cfg.platforms,
                    &inputs.work,
                ),
                PendingSource::Provider(provider) => resolve::provider_evidence(
                    oyzu,
                    provider,
                    &p.tags,
                    &cfg.platforms,
                    &inputs.work,
                ),
            };
            let doc = match result {
                Ok(d) => d,
                Err(e) => {
                    report
                        .verification_failed
                        .push(format!("{}: {e:#}", tool.origin));
                    continue;
                }
            };
            engine.get_or_insert_with(|| doc["engine"].clone());
            let mut by_version: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for record in doc["sets"].as_array().into_iter().flatten() {
                let normalized = record["version"]["normalized"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                by_version
                    .entry(normalized)
                    .or_default()
                    .push(record.clone());
            }
            for v in &mut tool.versions {
                if let Some(mut records) = by_version.remove(&v.version) {
                    records.sort_by(|a, b| a["platform"].as_str().cmp(&b["platform"].as_str()));
                    v.sets = records;
                    report.verified_this_run += 1;
                    if !index::verified(v) {
                        report.verification_failed.push(format!(
                            "{} {}: {}",
                            tool.origin,
                            v.version,
                            failure_reason(&v.sets)
                        ));
                    }
                }
            }
        }
    } else {
        report.deferred += pending.iter().map(|p| p.tags.len()).sum::<usize>();
    }

    // Records are kept for the newest versions only, which bounds the single
    // index file. An organization that approved an older version keeps the
    // digest it adopted; dropping records never adds trust, so it is routine.
    // A core provider that counts by release line keeps them per line.
    for tool in &mut tools {
        let core = tool
            .origin
            .strip_prefix("core:")
            .and_then(|p| cfg.core.get(p))
            .filter(|c| c.line.is_some());
        let keep = core
            .and_then(|c| c.keep_records)
            .unwrap_or(cfg.verified.keep_records);
        let mut kept: BTreeMap<String, usize> = BTreeMap::new();
        for v in tool.versions.iter_mut().filter(|v| !v.sets.is_empty()) {
            let line = core.and_then(|c| c.line_of(&v.version)).unwrap_or("");
            let n = kept.entry(line.to_owned()).or_default();
            if *n < keep {
                *n += 1;
            } else {
                v.sets.clear();
            }
        }
    }

    tools.sort_by(|a, b| a.id.cmp(&b.id));
    let engine = engine
        .or_else(|| inputs.previous.as_ref().map(|p| p.engine.clone()))
        .unwrap_or_else(|| {
            json!({
                "name": "oyzu",
                "version": ENGINE_FALLBACK_VERSION,
                "evaluator_digest": oyzu_recipe::evaluator_digest()
            })
        });
    report.tools = tools.len();
    report.versions = tools.iter().map(|t| t.versions.len()).sum();
    report.verified_records = tools
        .iter()
        .flat_map(|t| &t.versions)
        .flat_map(|v| &v.sets)
        .filter(|r| r["tier"] == "verified")
        .count();
    let index = Index {
        format: index::FORMAT,
        kind: index::KIND.into(),
        // Never stamped here: `stamp` adds these to the release copy only.
        serial: None,
        expires: None,
        engine,
        snapshot: Snapshot {
            source: cfg.aqua_registry.source.clone(),
            tag: Some(cfg.aqua_registry.tag.clone()),
            commit: cfg.aqua_registry.commit.clone(),
            form: snapshot.form.to_owned(),
            digest: cfg.aqua_registry.digest.clone(),
        },
        tools,
    };
    index.check()?;
    Ok((index, report))
}

#[cfg(test)]
mod tests {
    use super::{canonical_id, core_candidates, failure_reason, keep_lines};
    use crate::config::{CoreProvider, Verified};
    use crate::index::{DateSource, Released, ServingClass, Tool, Version};
    use serde_json::json;

    fn version(v: &str, verified: bool) -> Version {
        Version {
            version: v.into(),
            released: Released {
                time: "2025-09-01T00:00:00Z".into(),
                source: DateSource::Publisher,
            },
            sets: if verified {
                vec![json!({"tier": "verified", "status": "resolved"})]
            } else {
                vec![]
            },
        }
    }

    fn node(lines: Option<usize>) -> CoreProvider {
        CoreProvider {
            names: vec!["node".into()],
            max_versions: 2,
            verified: true,
            line: Some(1),
            lines,
            keep_records: Some(2),
        }
    }

    const VERIFIED: Verified = Verified {
        backfill: 1,
        max_versions_per_run: 150,
        keep_records: 10,
        include_prereleases: false,
    };

    #[test]
    fn release_lines_keep_their_own_versions_and_backfill() {
        let names = |vs: &[Version]| vs.iter().map(|v| v.version.clone()).collect::<Vec<_>>();
        let listed = [
            "24.9.0", "24.8.0", "24.7.0", "22.20.0", "22.19.0", "20.19.5",
        ];
        let mut versions: Vec<Version> = listed.iter().map(|v| version(v, false)).collect();
        keep_lines(&mut versions, &node(Some(2)));
        assert_eq!(names(&versions), ["24.9.0", "24.8.0", "22.20.0", "22.19.0"]);

        // Line 24 has records; line 22 was listed before but never got any.
        // By line, 22 gets its backfill while 24 verifies only its new release.
        let previous = Tool {
            id: "oyzu:node".into(),
            origin: "core:node".into(),
            serving_class: ServingClass::Direct,
            versions: vec![
                version("24.8.0", true),
                version("22.20.0", false),
                version("22.19.0", false),
            ],
        };
        let versions = vec![
            version("24.9.0", false),
            version("24.8.0", true),
            version("22.20.0", false),
            version("22.19.0", false),
        ];
        assert_eq!(
            core_candidates(&versions, Some(&previous), &node(None), &VERIFIED),
            ["24.9.0", "22.20.0"]
        );
        // Counted overall, the newer line's records leave 22 without any.
        let mut flat = node(None);
        flat.line = None;
        flat.keep_records = None;
        assert_eq!(
            core_candidates(&versions, Some(&previous), &flat, &VERIFIED),
            ["24.9.0"]
        );
    }

    #[test]
    fn a_failed_version_reports_its_first_failed_check() {
        let records = [json!({
            "platform": "linux/amd64/gnu",
            "verified": {"results": [
                {"evidence": "checksum#0", "outcome": "verified", "detail": null},
                {"evidence": "github-attestation#0", "outcome": "failed", "detail": "no inline bundle"}
            ]}
        })];
        assert_eq!(
            failure_reason(&records),
            "linux/amd64/gnu github-attestation#0: no inline bundle"
        );
        assert_eq!(failure_reason(&[]), "no record reached the verified tier");
    }

    #[test]
    fn ids_use_the_curated_name_then_the_package() {
        assert_eq!(canonical_id("cli/cli", Some("gh")), "oyzu:gh");
        assert_eq!(
            canonical_id("BurntSushi/ripgrep", None),
            "oyzu:burntsushi/ripgrep"
        );
    }
}
