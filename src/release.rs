//! The release step (Sigstore signing contract, decided 2026-10-10).
//!
//! A release of `oyzuai/tool-index` is tag `index-v<serial>`, marked latest,
//! with four assets in one flat directory: the stamped `index.json`, the same
//! bytes compressed with zstd (`index.json.zst`, which the CLI fetches; the
//! signature is over the decompressed bytes), its cosign bundle
//! `index.json.sigstore.json` and the registry's `NOTICE`. This
//! module checks a release directory and prints the plan; the workflow
//! publishes it with `gh release create` after `cosign verify-blob-attestation`.
use crate::index::Index;
use anyhow::{bail, ensure, Context, Result};
use base64::Engine as _;
use jiff::Timestamp;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const INDEX: &str = "index.json";
pub const COMPRESSED: &str = "index.json.zst";
pub const BUNDLE: &str = "index.json.sigstore.json";
pub const NOTICE: &str = "NOTICE";
/// Every asset of a release, in upload order.
pub const ASSETS: [&str; 4] = [INDEX, COMPRESSED, BUNDLE, NOTICE];

pub const BUNDLE_MEDIA_TYPE: &str = "application/vnd.dev.sigstore.bundle.v0.3+json";
pub const PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";

#[derive(Debug, Serialize)]
pub struct Plan {
    pub tag: String,
    pub serial: u64,
    pub expires: String,
    pub assets: Vec<Asset>,
    /// Assets not in the directory yet; a non-dry run refuses when any are.
    pub missing: Vec<String>,
    /// The command the release workflow runs, for review.
    pub command: String,
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct Asset {
    pub name: String,
    pub bytes: u64,
}

/// Checks a release directory and returns what would be published.
///
/// The stamped index must be valid and unexpired at `now`. When the bundle is
/// present, its statement must name `index.json`'s SHA-256 (the signature
/// itself is checked by `cosign verify-blob-attestation` in the workflow). Without
/// `dry_run`, every asset must exist.
pub fn plan(dir: &Path, now: Timestamp, dry_run: bool) -> Result<Plan> {
    let index_bytes = std::fs::read(dir.join(INDEX))
        .with_context(|| format!("reading {}", dir.join(INDEX).display()))?;
    let index: Index = serde_json::from_slice(&index_bytes).context("parsing index.json")?;
    index.check()?;
    let (Some(serial), Some(expires)) = (index.serial, index.expires.clone()) else {
        bail!("index.json is not stamped; run `tool-index stamp` first");
    };
    let expiry: Timestamp = expires.parse().context("index.json expires")?;
    ensure!(expiry > now, "index.json expired at {expires}");

    let mut assets = Vec::new();
    let mut missing = Vec::new();
    for name in ASSETS {
        match std::fs::metadata(dir.join(name)) {
            Ok(m) if m.is_file() => assets.push(Asset {
                name: name.into(),
                bytes: m.len(),
            }),
            _ => missing.push(name.to_string()),
        }
    }
    if !missing.contains(&COMPRESSED.to_string()) {
        check_compressed(&std::fs::read(dir.join(COMPRESSED))?, &index_bytes)?;
    }
    if !missing.contains(&BUNDLE.to_string()) {
        check_bundle(&std::fs::read(dir.join(BUNDLE))?, &index_bytes)?;
    }
    if !dry_run && !missing.is_empty() {
        bail!("{} is missing {}", dir.display(), missing.join(", "));
    }
    let tag = format!("index-v{serial}");
    let files: Vec<String> = ASSETS
        .iter()
        .map(|a| dir.join(a).display().to_string())
        .collect();
    Ok(Plan {
        command: format!(
            "gh release create {tag} --latest --title {tag} --notes 'Index serial {serial}, expires {expires}' {}",
            files.join(" ")
        ),
        tag,
        serial,
        expires,
        assets,
        missing,
        dry_run,
    })
}

/// `index.json.zst` decompresses to exactly the stamped `index.json`.
pub fn check_compressed(compressed: &[u8], index: &[u8]) -> Result<()> {
    use std::io::Read as _;
    let decoder = ruzstd::decoding::StreamingDecoder::new(compressed)
        .map_err(|e| anyhow::anyhow!("{COMPRESSED} is not zstd: {e}"))?;
    let mut bytes = Vec::new();
    decoder
        .take(index.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("decompressing {COMPRESSED}"))?;
    ensure!(
        bytes == index,
        "{COMPRESSED} does not decompress to index.json"
    );
    Ok(())
}

/// The bundle is a v0.3 DSSE bundle whose in-toto statement names the
/// index's exact bytes. This catches a bundle for a different file before
/// anything is uploaded; it does not verify the signature.
pub fn check_bundle(bundle: &[u8], index: &[u8]) -> Result<()> {
    let bundle: Value = serde_json::from_slice(bundle).context("parsing the bundle")?;
    ensure!(
        bundle["mediaType"] == BUNDLE_MEDIA_TYPE,
        "the bundle is not {BUNDLE_MEDIA_TYPE}"
    );
    let envelope = &bundle["dsseEnvelope"];
    ensure!(
        envelope["payloadType"] == PAYLOAD_TYPE,
        "the bundle's DSSE payload is not an in-toto statement"
    );
    let payload = base64::engine::general_purpose::STANDARD
        .decode(envelope["payload"].as_str().unwrap_or_default())
        .context("decoding the DSSE payload")?;
    let statement: Value = serde_json::from_slice(&payload).context("parsing the statement")?;
    let digest = hex::encode(Sha256::digest(index));
    let names_index = statement["subject"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|s| s["digest"]["sha256"] == digest.as_str());
    ensure!(
        names_index,
        "the bundle does not sign index.json (sha256 {digest})"
    );
    Ok(())
}
