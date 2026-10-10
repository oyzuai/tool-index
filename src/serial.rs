//! Which release the next serial follows.
//!
//! Releases are tagged `index-v<serial>`. The next serial comes from the
//! highest such tag, not from whichever release GitHub marks "latest", which a
//! person can move. Serials run 1, 2, 3 with no gap: a gap or a tag that is
//! not `index-v<number>` means something published outside this workflow, and
//! the release stops rather than guess.
use anyhow::{bail, ensure, Result};
use std::collections::BTreeSet;

pub const TAG_PREFIX: &str = "index-v";

/// The highest serial among `tags` (other tags are ignored), or `None` before
/// the first release. Fails on a malformed `index-v` tag, a repeated serial or
/// a gap.
pub fn latest<S: AsRef<str>>(tags: &[S]) -> Result<Option<u64>> {
    let mut serials = BTreeSet::new();
    for tag in tags.iter().map(AsRef::as_ref).map(str::trim) {
        let Some(n) = tag.strip_prefix(TAG_PREFIX) else {
            continue;
        };
        let serial: u64 = match n.parse() {
            Ok(s) if s > 0 && !n.starts_with('0') => s,
            _ => bail!("tag {tag} is not {TAG_PREFIX}<serial>"),
        };
        ensure!(serials.insert(serial), "serial {serial} is tagged twice");
    }
    let Some(&highest) = serials.last() else {
        return Ok(None);
    };
    ensure!(
        serials.len() as u64 == highest,
        "release serials have a gap: {} tags up to {TAG_PREFIX}{highest}, missing {}",
        serials.len(),
        (1..highest)
            .filter(|s| !serials.contains(s))
            .map(|s| format!("{TAG_PREFIX}{s}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(Some(highest))
}

#[cfg(test)]
mod tests {
    use super::latest;

    #[test]
    fn the_highest_contiguous_serial_wins() {
        assert_eq!(latest::<&str>(&[]).unwrap(), None);
        assert_eq!(latest(&["v1.0", "index-v1", "index-v2"]).unwrap(), Some(2));
        assert_eq!(
            latest(&["index-v2", "index-v1", "index-v3"]).unwrap(),
            Some(3)
        );
    }

    #[test]
    fn gaps_and_odd_tags_stop_the_release() {
        assert!(latest(&["index-v1", "index-v3"])
            .unwrap_err()
            .to_string()
            .contains("index-v2"));
        assert!(latest(&["index-v1", "index-vx"]).is_err());
        assert!(latest(&["index-v01"]).is_err());
        assert!(latest(&["index-v0"]).is_err());
    }
}
