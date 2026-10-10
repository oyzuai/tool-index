//! Stamping the release copy (Sigstore signing contract, decided 2026-10-10).
//!
//! The committed `index/index.json` is reviewed through pull requests and
//! never carries `serial` or `expires`. At release time the workflow stamps a
//! copy: `serial` is the published serial plus one (1 for the first release)
//! and `expires` is the release time plus `signing.expires_days`. That copy is
//! what cosign signs and what clients check for expiry and rollback.
use crate::index::Index;
use anyhow::{bail, ensure, Context, Result};
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

/// Stamps `unstamped` (the committed index) for release.
///
/// `published` is the `index.json` of the latest release, if any; only its
/// `serial` is read. `now` is truncated to whole seconds so `expires` reads
/// as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn stamp(
    unstamped: &[u8],
    published: Option<&[u8]>,
    now: Timestamp,
    expires_days: i64,
) -> Result<Vec<u8>> {
    ensure!(
        (1..=366).contains(&expires_days),
        "signing.expires_days must be between 1 and 366"
    );
    let mut index: Index = serde_json::from_slice(unstamped).context("parsing the index")?;
    if index.stamped() {
        bail!("the index already carries serial or expires; stamp the unstamped committed copy");
    }
    index.check()?;
    let serial = match published {
        None => 1,
        Some(bytes) => {
            let doc: Value =
                serde_json::from_slice(bytes).context("parsing the published index")?;
            let Some(previous) = doc.get("serial").and_then(Value::as_u64) else {
                bail!("the published index has no serial");
            };
            previous
                .checked_add(1)
                .context("the published serial cannot grow")?
        }
    };
    let now = Timestamp::from_second(now.as_second())?;
    let expires = now.checked_add(SignedDuration::from_hours(24 * expires_days))?;
    index.serial = Some(serial);
    index.expires = Some(expires.to_string());
    index.to_bytes()
}
