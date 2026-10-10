//! Assembling `index.json` from the pinned snapshot, the version listing, the
//! previous index and, for the verified tier, `oyzu tools resolve` and
//! `oyzu tools evidence`.
//!
//! The previous index is the job's only state: it carries first-seen dates
//! and already-verified records forward, so a record is downloaded and checked
//! once, when its version first appears.
use crate::attempts::{Attempts, Due};
use crate::config::Config;
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
    /// Versions that failed before (`index/attempts.json`).
    pub attempts: Attempts,
    /// The oyzu revision `oyzu` was built from; a change retries failures.
    pub engine_rev: Option<String>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub tools: usize,
    pub versions: usize,
    pub verified_records: usize,
    pub verified_this_run: usize,
    pub verification_failed: Vec<String>,
    /// Versions over this run's budget, left for the next run.
    pub deferred: usize,
    /// Versions that failed recently and wait for their retry day.
    pub waiting_retry: usize,
    pub listing_errors: Vec<String>,
    /// Source archives found and hashed this run (`crate::sources`).
    pub sources_recorded: usize,
    /// Versions of `source_mirror` tools whose source archive was not found;
    /// a proxy serves them directly from upstream.
    pub source_failures: Vec<String>,
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
/// a version must start with a number (`1.2.3`, `2025-08-13T08-35-41Z`), or
/// be a word, a dot and a number, as MinIO tags every release
/// (`RELEASE.2025-08-13T08-35-41Z`).
fn looks_like_version(normalized: &str) -> bool {
    let start = index::word_prefix(normalized).map_or(normalized, |(_, rest)| rest);
    start.bytes().next().is_some_and(|b| b.is_ascii_digit())
        && !normalized.contains('/')
        && !normalized.contains('@')
}

/// Pre-releases kept per tool, beside `discovery.max_versions` releases.
const MAX_PRERELEASES: usize = 5;

/// Keeps the newest `max` versions, of which at most [`MAX_PRERELEASES`]
/// pre-releases. flyctl tags thousands of `v2026.9.26-pr5241.1` preview
/// builds that sort above its `v0.4.115` releases and would otherwise fill
/// the list.
fn keep_newest(versions: &mut Vec<Version>, max: usize) {
    let mut pre = 0;
    versions.retain(|v| {
        if index::prerelease(&v.version) {
            pre += 1;
            pre <= MAX_PRERELEASES
        } else {
            true
        }
    });
    versions.truncate(max);
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
            source: None,
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
    /// The normalized version of each tag, in the same order.
    versions: Vec<String>,
}

/// A version the run would like to verify, before the budget is applied.
struct Want {
    due: Due,
    /// Core providers first, then the launch set in file order.
    rank: usize,
    tool: usize,
    tag: String,
    version: String,
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
/// A version counts as done once every platform is settled
/// ([`index::complete`]), so one whose Linux records failed is retried.
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
            !index::complete(v)
                && (*i < cfg.backfill || (!first_run && !known.contains(v.version.as_str())))
        })
        .map(|(_, v)| v.version.clone())
        .collect()
}

pub fn build(inputs: Inputs<'_>) -> Result<(Index, Report, Attempts)> {
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
    let mut attempts = inputs.attempts.clone();
    let engine_rev = inputs.engine_rev.as_deref();
    let due = |origin: &str, version: &str| {
        attempts.due(
            origin,
            version,
            inputs.now,
            engine_rev,
            cfg.verified.retry_after_days,
        )
    };
    // Budget order: core providers, then the launch set as the file lists it
    // (its groups put the most used tools first), not registry order.
    let ranks: BTreeMap<String, usize> = cfg
        .core
        .keys()
        .map(|p| format!("core:{p}"))
        .chain(
            inputs
                .config
                .launch_set
                .tools
                .iter()
                .map(|t| aqua_id(&t.package)),
        )
        .enumerate()
        .map(|(i, o)| (o, i))
        .collect();
    let rank_of = |origin: &str| ranks.get(origin).copied().unwrap_or(usize::MAX);
    let mut wants: Vec<Want> = Vec::new();
    let mut sources: BTreeMap<usize, PendingSource> = BTreeMap::new();

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
        versions.truncate(core.max_versions);
        let id = format!(
            "oyzu:{}",
            core.names.first().map(String::as_str).unwrap_or(provider)
        );
        ids.insert(id.clone());
        if core.verified {
            // Records come from `oyzu tools evidence --provider`, with the
            // record id `core:<provider>` the index carries as the origin.
            for v in candidates(&versions, previous(&origin), &cfg.verified) {
                wants.push(Want {
                    due: due(&origin, &v),
                    rank: rank_of(&origin),
                    tool: tools.len(),
                    tag: v.clone(),
                    version: v,
                });
            }
            sources.insert(tools.len(), PendingSource::Provider(provider.clone()));
        } else {
            for v in &mut versions {
                v.sets.clear();
            }
        }
        tools.push(Tool {
            id,
            origin,
            serving_class: serving.class,
            source_mirror: serving.source_mirror,
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
        keep_newest(&mut versions, cfg.discovery.max_versions);
        if !is_launch {
            for v in &mut versions {
                v.sets.clear();
            }
        } else {
            // Verify versions with no verified records yet: on a tool's first
            // run only the newest few, afterwards every new version.
            for v in candidates(&versions, prev, &cfg.verified) {
                if let Some(tag) = tag_of.get(&v) {
                    wants.push(Want {
                        due: due(&origin, &v),
                        rank: rank_of(&origin),
                        tool: tools.len(),
                        tag: tag.clone(),
                        version: v,
                    });
                }
            }
            sources.insert(tools.len(), PendingSource::Package(package.name.clone()));
        }
        tools.push(Tool {
            id,
            origin,
            serving_class: serving.class,
            source_mirror: serving.source_mirror,
            versions,
        });
    }

    // New versions first (a tool's first records, then new releases), then
    // retries that are due; within each, core providers and the launch set in
    // order, newest version first. Recent failures wait for their retry day.
    report.waiting_retry = wants.iter().filter(|w| w.due == Due::Wait).count();
    wants.retain(|w| w.due != Due::Wait);
    wants.sort_by_key(|w| (w.due, w.rank));
    report.deferred += wants
        .len()
        .saturating_sub(cfg.verified.max_versions_per_run);
    wants.truncate(cfg.verified.max_versions_per_run);
    let mut pending: Vec<Pending> = Vec::new();
    for w in wants {
        match pending.iter_mut().find(|p| p.tool == w.tool) {
            Some(p) => {
                p.tags.push(w.tag);
                p.versions.push(w.version);
            }
            None => pending.push(Pending {
                tool: w.tool,
                source: match &sources[&w.tool] {
                    PendingSource::Package(p) => PendingSource::Package(p.clone()),
                    PendingSource::Provider(p) => PendingSource::Provider(p.clone()),
                },
                tags: vec![w.tag],
                versions: vec![w.version],
            }),
        }
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
                    let reason: String = format!("{e:#}").chars().take(300).collect();
                    for v in &p.versions {
                        attempts.failed(&tool.origin, v, inputs.now, engine_rev, reason.clone());
                    }
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
            let mut updated = BTreeSet::new();
            for v in &mut tool.versions {
                if let Some(mut records) = by_version.remove(&v.version) {
                    updated.insert(v.version.clone());
                    // A platform that verified before keeps its published
                    // record; a retry only fills in the ones that did not.
                    for r in &mut records {
                        if let Some(old) = v
                            .sets
                            .iter()
                            .find(|o| o["platform"] == r["platform"] && index::record_verified(o))
                        {
                            *r = old.clone();
                        }
                    }
                    records.sort_by(|a, b| a["platform"].as_str().cmp(&b["platform"].as_str()));
                    v.sets = records;
                    report.verified_this_run += 1;
                    if index::complete(v) {
                        attempts.clear(&tool.origin, &v.version);
                    } else {
                        let reason = failure_reason(&v.sets);
                        report
                            .verification_failed
                            .push(format!("{} {}: {reason}", tool.origin, v.version));
                        attempts.failed(&tool.origin, &v.version, inputs.now, engine_rev, reason);
                    }
                }
            }
            for v in &p.versions {
                if !updated.contains(v) {
                    attempts.failed(
                        &tool.origin,
                        v,
                        inputs.now,
                        engine_rev,
                        "oyzu returned no record for this version".into(),
                    );
                }
            }
        }
    } else {
        report.deferred += pending.iter().map(|p| p.tags.len()).sum::<usize>();
    }

    // Records are kept for the newest versions only, which bounds the single
    // index file. An organization that approved an older version keeps the
    // digest it adopted; dropping records never adds trust, so it is routine.
    for tool in &mut tools {
        let mut kept = 0;
        for v in tool.versions.iter_mut().filter(|v| !v.sets.is_empty()) {
            if kept < cfg.verified.keep_records {
                kept += 1;
            } else {
                v.sets.clear();
            }
        }
        // A source archive goes with the records it backs, and only for a
        // tool that still needs one.
        for v in &mut tool.versions {
            if !tool.source_mirror || v.sets.is_empty() {
                v.source = None;
            }
        }
    }

    // Forget failures for versions that are verified now or left the index.
    {
        let by_origin: BTreeMap<&str, &Tool> =
            tools.iter().map(|t| (t.origin.as_str(), t)).collect();
        attempts.retain(|origin, version| {
            by_origin.get(origin).is_some_and(|t| {
                t.versions
                    .iter()
                    .any(|v| v.version == version && !index::complete(v))
            }) || inputs.only.as_ref().is_some_and(|o| !o.contains(origin))
        });
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
    Ok((index, report, attempts))
}

#[cfg(test)]
mod tests {
    use super::{canonical_id, failure_reason, keep_newest, looks_like_version};
    use crate::index::Version;
    use serde_json::json;

    #[test]
    fn preview_builds_do_not_crowd_out_releases() {
        let mut versions: Vec<Version> = (0..40)
            .map(|i| format!("2026.9.{i}-pr5241.1"))
            .chain(["0.4.115".to_owned(), "0.4.114".to_owned()])
            .map(|v| {
                let released = json!({"time": "2026-10-01", "source": "publisher"});
                serde_json::from_value(json!({"version": v, "released": released, "sets": []}))
                    .unwrap()
            })
            .collect();
        keep_newest(&mut versions, 6);
        let kept: Vec<_> = versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(kept.len(), 6);
        assert_eq!(&kept[5..], ["0.4.115"]);
    }

    #[test]
    fn release_tags_look_like_versions_and_others_do_not() {
        for v in [
            "1.2.3",
            "2025-08-13T08-35-41Z",
            "RELEASE.2025-08-13T08-35-41Z",
        ] {
            assert!(looks_like_version(v), "{v}");
        }
        for v in [
            "nightly-2025",
            "chart-3.9.1",
            "pkg/v3.1.0",
            "@biomejs/biome@2.0.0",
            "lsp.x",
        ] {
            assert!(!looks_like_version(v), "{v}");
        }
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
