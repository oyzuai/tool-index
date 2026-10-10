//! Versions that failed verification, so a run does not retry them every day.
//!
//! The index carries records but not why a version stayed at the discovery
//! tier or when it was last tried, so the job keeps that beside it in
//! `index/attempts.json`. A failed version is retried after
//! `verified.retry_after_days`, or at once when the job runs a different oyzu
//! (a fix there, such as reading `bundle_url` attestations, is the usual way a
//! failure clears). Until then it spends none of the run's budget.
use anyhow::{Context, Result};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempts {
    pub format: u32,
    /// By origin (`aqua:<package>`, `core:<provider>`), then normalized version.
    pub tools: BTreeMap<String, BTreeMap<String, Attempt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub first_failed: String,
    pub last_attempt: String,
    pub attempts: u32,
    /// The oyzu revision that ran the last attempt, when the job knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// The first failed check, or why no record came back.
    pub reason: String,
}

/// Whether a version is worth spending budget on this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Due {
    /// Never tried, or tried before this file existed: first in line.
    New,
    /// Failed before and its wait is over, or the oyzu revision changed.
    Retry,
    /// Failed recently with the same oyzu; skipped this run.
    Wait,
}

impl Attempts {
    pub fn read(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                format: FORMAT,
                ..Self::default()
            });
        }
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let a: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        anyhow::ensure!(
            a.format == FORMAT,
            "{} has format {}, expected {FORMAT}",
            path.display(),
            a.format
        );
        Ok(a)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn get(&self, origin: &str, version: &str) -> Option<&Attempt> {
        self.tools.get(origin)?.get(version)
    }

    pub fn due(
        &self,
        origin: &str,
        version: &str,
        now: Timestamp,
        engine: Option<&str>,
        retry_after_days: i64,
    ) -> Due {
        let Some(a) = self.get(origin, version) else {
            return Due::New;
        };
        if engine.is_some() && a.engine.as_deref() != engine {
            return Due::Retry;
        }
        let last: Option<Timestamp> = a.last_attempt.parse().ok();
        match last.and_then(|l| {
            l.checked_add(SignedDuration::from_hours(24 * retry_after_days))
                .ok()
        }) {
            Some(next) if next > now => Due::Wait,
            _ => Due::Retry,
        }
    }

    pub fn failed(
        &mut self,
        origin: &str,
        version: &str,
        now: Timestamp,
        engine: Option<&str>,
        reason: String,
    ) {
        let now = now.to_string();
        let entry = self
            .tools
            .entry(origin.to_owned())
            .or_default()
            .entry(version.to_owned())
            .or_insert_with(|| Attempt {
                first_failed: now.clone(),
                last_attempt: now.clone(),
                attempts: 0,
                engine: None,
                reason: String::new(),
            });
        entry.last_attempt = now;
        entry.attempts += 1;
        entry.engine = engine.map(str::to_owned);
        entry.reason = reason;
    }

    pub fn clear(&mut self, origin: &str, version: &str) {
        if let Some(t) = self.tools.get_mut(origin) {
            t.remove(version);
            if t.is_empty() {
                self.tools.remove(origin);
            }
        }
    }

    /// Keeps only entries `keep` accepts (versions still listed and unverified).
    pub fn retain(&mut self, mut keep: impl FnMut(&str, &str) -> bool) {
        for (origin, versions) in &mut self.tools {
            versions.retain(|v, _| keep(origin, v));
        }
        self.tools.retain(|_, v| !v.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn a_failure_waits_for_its_retry_day_or_a_new_engine() {
        let mut a = Attempts::default();
        let t0 = at("2026-10-10T03:00:00Z");
        assert_eq!(a.due("aqua:x/y", "1.0", t0, Some("r1"), 7), Due::New);
        a.failed("aqua:x/y", "1.0", t0, Some("r1"), "upstream 404".into());
        let day = |d: i64| t0.checked_add(SignedDuration::from_hours(24 * d)).unwrap();
        assert_eq!(a.due("aqua:x/y", "1.0", day(1), Some("r1"), 7), Due::Wait);
        assert_eq!(a.due("aqua:x/y", "1.0", day(7), Some("r1"), 7), Due::Retry);
        assert_eq!(a.due("aqua:x/y", "1.0", day(1), Some("r2"), 7), Due::Retry);
        // Without a known engine only the clock counts.
        assert_eq!(a.due("aqua:x/y", "1.0", day(1), None, 7), Due::Wait);
        a.failed("aqua:x/y", "1.0", day(7), Some("r1"), "still 404".into());
        let e = a.get("aqua:x/y", "1.0").unwrap();
        assert_eq!(
            (e.attempts, e.first_failed.as_str()),
            (2, "2026-10-10T03:00:00Z")
        );
        a.clear("aqua:x/y", "1.0");
        assert!(a.tools.is_empty());
    }
}
