//! Deciding whether the job's pull request may merge on its own (decided
//! 2026-10-09): routine version additions for indexed tools merge
//! automatically; a new tool or a new download host waits for a person.
//!
//! Anything else that changes what clients trust also waits: a new registry
//! snapshot or resolve engine, a changed origin or serving class, a removed tool or
//! version, a version withdrawn or no longer withdrawn (`config/withdrawn.toml`),
//! and above all a changed record for a version already verified.
//!
//! The release fields `serial` and `expires` are ignored, so a daily re-stamp
//! never needs review.
use crate::config::Withdrawals;
use crate::index::{record_hosts, record_verified, verified, Index, Tool, Version};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    Routine,
    Review,
    NoChange,
}

#[derive(Debug, Serialize)]
pub struct Review {
    pub decision: Decision,
    /// Why a person must review; empty when routine.
    pub reasons: Vec<String>,
    pub added_versions: usize,
    pub added_records: usize,
    pub tools_with_new_versions: Vec<String>,
    /// For the pull request body; they do not change the decision.
    pub notes: Vec<String>,
}

/// The reason line for a change in a version's `withdrawn`, if any.
fn withdrawal_change(id: &str, old: Option<&Version>, new: &Version) -> Option<String> {
    let before = old.and_then(|o| o.withdrawn.as_ref());
    match (before, &new.withdrawn) {
        (None, Some(w)) => Some(format!("{id} {} is withdrawn: {}", new.version, w.reason)),
        (Some(_), None) => Some(format!("{id} {} is no longer withdrawn", new.version)),
        (Some(a), Some(b)) if a != b => Some(format!(
            "{id} {} changes its withdrawal reason to: {}",
            new.version, b.reason
        )),
        _ => None,
    }
}

fn hosts(tool: &Tool) -> BTreeSet<String> {
    tool.versions
        .iter()
        .flat_map(|v| &v.sets)
        .flat_map(record_hosts)
        .collect()
}

pub fn review(previous: Option<&Index>, next: &Index) -> Review {
    review_with(previous, next, &Withdrawals::default())
}

/// [`review`], also noting withdrawals (`config/withdrawn.toml`) that name a
/// tool or version `next` does not list: the build ignores them, so a person
/// should correct or remove the entry.
pub fn review_with(previous: Option<&Index>, next: &Index, withdrawals: &Withdrawals) -> Review {
    let mut r = decide(previous, next);
    r.notes.extend(
        withdrawals
            .not_indexed(next)
            .into_iter()
            .map(|w| format!("config/withdrawn.toml names {w}, which the index does not list")),
    );
    r
}

fn decide(previous: Option<&Index>, next: &Index) -> Review {
    let mut reasons = Vec::new();
    let mut added_versions = 0;
    let mut added_records = 0;
    let mut touched = BTreeSet::new();
    let Some(prev) = previous else {
        let mut reasons = vec!["first index: every tool is new".to_owned()];
        for t in &next.tools {
            reasons.extend(
                t.versions
                    .iter()
                    .filter_map(|v| withdrawal_change(&t.id, None, v)),
            );
        }
        return Review {
            decision: Decision::Review,
            reasons,
            added_versions: next.tools.iter().map(|t| t.versions.len()).sum(),
            added_records: next
                .tools
                .iter()
                .flat_map(|t| &t.versions)
                .map(|v| v.sets.len())
                .sum(),
            tools_with_new_versions: next
                .tools
                .iter()
                .filter(|t| !t.versions.is_empty())
                .map(|t| t.id.clone())
                .collect(),
            notes: Vec::new(),
        };
    };
    if prev.snapshot != next.snapshot {
        reasons.push(format!(
            "registry snapshot changes from {} to {}",
            prev.snapshot
                .tag
                .as_deref()
                .unwrap_or(&prev.snapshot.digest),
            next.snapshot
                .tag
                .as_deref()
                .unwrap_or(&next.snapshot.digest)
        ));
    }
    if prev.engine["evaluator_digest"] != next.engine["evaluator_digest"] {
        reasons.push("the resolve engine's evaluator changed".into());
    }
    let before: BTreeMap<&str, &Tool> = prev.tools.iter().map(|t| (t.id.as_str(), t)).collect();
    let after: BTreeMap<&str, &Tool> = next.tools.iter().map(|t| (t.id.as_str(), t)).collect();
    for id in before.keys().filter(|id| !after.contains_key(*id)) {
        reasons.push(format!("{id} is removed"));
    }
    for (id, new) in &after {
        let Some(old) = before.get(id) else {
            reasons.push(format!("{id} is a new tool"));
            continue;
        };
        if old.origin != new.origin {
            reasons.push(format!(
                "{id} changes origin from {} to {}",
                old.origin, new.origin
            ));
        }
        if old.serving_class != new.serving_class {
            reasons.push(format!(
                "{id} changes serving class from {:?} to {:?}",
                old.serving_class, new.serving_class
            ));
        }
        let old_hosts = hosts(old);
        for host in hosts(new).difference(&old_hosts) {
            reasons.push(format!("{id} downloads from a new host, {host}"));
        }
        let old_versions: BTreeMap<&str, _> = old
            .versions
            .iter()
            .map(|v| (v.version.as_str(), v))
            .collect();
        let new_versions: BTreeSet<&str> =
            new.versions.iter().map(|v| v.version.as_str()).collect();
        for v in old_versions.keys().filter(|v| !new_versions.contains(*v)) {
            // Discovery tools keep only their newest versions; dropping the
            // oldest is routine. A verified version disappearing is not.
            if verified(old_versions[v]) {
                reasons.push(format!("{id} {v} is removed after it was verified"));
            } else if old_versions[v].withdrawn.is_some() {
                reasons.push(format!("{id} {v} is removed while withdrawn"));
            }
        }
        for v in &new.versions {
            let old = old_versions.get(v.version.as_str()).copied();
            if let Some(change) = withdrawal_change(id, old, v) {
                reasons.push(change);
            }
            match old {
                None => {
                    added_versions += 1;
                    added_records += v.sets.len();
                    touched.insert(id.to_string());
                }
                Some(o) => {
                    if o.source.is_some() && o.source != v.source {
                        reasons.push(format!("{id} {} changes its source archive", v.version));
                    }
                    if o.released != v.released {
                        reasons.push(format!("{id} {} changes its release date", v.version));
                    }
                    // Records age out of the index (`keep_records`); dropping
                    // them removes information and adds no trust. A platform
                    // that did not reach the verified tier is retried
                    // (`index/attempts.json`), and its new record is judged
                    // like a new version's: new hosts still need review.
                    if v.sets.is_empty() || o.sets == v.sets {
                        continue;
                    }
                    // Per platform: a record that was verified must stay as
                    // published; any other may be replaced, and counts as
                    // added.
                    let mut changed_verified = false;
                    for r in &v.sets {
                        match o.sets.iter().find(|x| x["platform"] == r["platform"]) {
                            Some(x) if x == r => {}
                            Some(x) if record_verified(x) => changed_verified = true,
                            _ => added_records += 1,
                        }
                    }
                    changed_verified |= o.sets.iter().any(|x| {
                        record_verified(x) && !v.sets.iter().any(|r| r["platform"] == x["platform"])
                    });
                    if changed_verified {
                        reasons.push(format!(
                            "{id} {} changes records that were already published",
                            v.version
                        ));
                    }
                }
            }
        }
    }
    // `serial` and `expires` are stamped at release time and change on every
    // re-sign, so they never make a change (see `crate::stamp`).
    let changed = added_versions > 0
        || added_records > 0
        || !reasons.is_empty()
        || prev.unstamped() != next.unstamped();
    Review {
        decision: if !reasons.is_empty() {
            Decision::Review
        } else if changed {
            Decision::Routine
        } else {
            Decision::NoChange
        },
        reasons,
        added_versions,
        added_records,
        tools_with_new_versions: touched.into_iter().collect(),
        notes: Vec::new(),
    }
}

/// The pull request body. A person reviewing reads the reasons first.
pub fn markdown(r: &Review) -> String {
    let mut s = String::new();
    match r.decision {
        Decision::Routine => s.push_str(
            "Routine update: new versions of tools already in the index, from hosts already in the index. This merges automatically once checks pass.\n\n",
        ),
        Decision::Review => s.push_str("This update needs a person to review it before it merges, because:\n\n"),
        Decision::NoChange => s.push_str("No change.\n"),
    }
    for reason in &r.reasons {
        s.push_str(&format!("- {reason}\n"));
    }
    if r.decision != Decision::NoChange {
        s.push_str(&format!(
            "\n{} new versions and {} new records across {} tools.\n",
            r.added_versions,
            r.added_records,
            r.tools_with_new_versions.len()
        ));
    }
    if !r.notes.is_empty() {
        s.push_str("\nNotes:\n\n");
        for note in &r.notes {
            s.push_str(&format!("- {note}\n"));
        }
    }
    s
}
