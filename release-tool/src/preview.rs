//! Finite local Preview metadata preparation. This owner never signs, verifies
//! cryptography, downloads or publishes. The operator verifies the exact prior
//! published bytes with the approved human identity before supplying them here.
use crate::{plugins, release};
use anyhow::{ensure, Context, Result};
use base64::Engine as _;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub const CATALOG: &str = "plugins-preview.json";
pub const BUNDLE: &str = "plugins-preview.json.sigstore.json";
pub const TAG: &str = "plugins-preview";
const EXPIRY_SECONDS: i64 = 14 * 24 * 3600;
const MAX_ROOT: usize = 1024 * 1024;
const INDEX: &str = "application/vnd.oci.image.index.v1+json";
const RELEASE: &str = "application/vnd.oyzu.plugin.release.v1alpha1";

/// The public OCI root shape, not an independent payload/graph verifier.
/// Unknown OCI fields remain allowed by its extensible contract.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Root {
    schema_version: u64,
    media_type: String,
    artifact_type: String,
    manifests: Vec<Descriptor>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    media_type: String,
    digest: String,
    size: u64,
}

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn check_content(content: &plugins::CatalogContent) -> Result<()> {
    content.check()?;
    ensure!(
        content.authority == plugins::OFFICIAL_PUBLISHER,
        "Preview catalog authority must be oyzu"
    );
    for package in &content.packages {
        ensure!(
            package.publisher == plugins::OFFICIAL_PUBLISHER,
            "Preview cannot name another publisher"
        );
        ensure!(
            package.releases.iter().all(|r| r.preview),
            "Preview cannot advertise stable releases"
        );
    }
    for withdrawal in &content.withdrawals {
        ensure!(
            withdrawal
                .package
                .split_once('/')
                .is_some_and(|(p, _)| p == plugins::OFFICIAL_PUBLISHER),
            "Preview cannot withdraw another publisher's release"
        );
        let encoded = withdrawal
            .root_preimage
            .as_deref()
            .context("Preview withdrawal requires root_preimage")?;
        ensure!(
            encoded.len() <= 1_398_104,
            "Preview root proof exceeds bound"
        );
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
        ensure!(
            bytes.len() <= MAX_ROOT && sha(&bytes) == withdrawal.root_digest,
            "Preview withdrawal digest must hash the exact raw root preimage"
        );
        let root: Root = serde_json::from_slice(&bytes).context("Preview withdrawal OCI root")?;
        ensure!(
            root.schema_version == 2
                && root.media_type == INDEX
                && root.artifact_type == RELEASE
                && root.manifests.len() <= 33,
            "Preview withdrawal requires the release-root shape"
        );
        for descriptor in &root.manifests {
            ensure!(
                descriptor.media_type == "application/vnd.oci.image.manifest.v1+json"
                    && (1..=MAX_ROOT as u64).contains(&descriptor.size)
                    && descriptor
                        .digest
                        .strip_prefix("sha256:")
                        .is_some_and(|d| d.len() == 64
                            && d.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
                "Preview root contains an invalid descriptor"
            );
        }
        ensure!(
            root.annotations
                .get("dev.oyzu.plugin.authority")
                .map(String::as_str)
                == Some("local-preview")
                && root.annotations.get("org.opencontainers.image.title")
                    == Some(&withdrawal.package)
                && root.annotations.get("org.opencontainers.image.version")
                    == Some(&withdrawal.version),
            "Preview withdrawal must prove the immutable authority and exact package/version"
        );
    }
    Ok(())
}

/// `published` MUST be the last independently cryptographically verified
/// Preview document, or None only before the first publication. This helper
/// validates bytes/shape and succession, not that external verification.
pub fn generate(
    source: &str,
    verified: Option<&[u8]>,
    published: Option<&[u8]>,
    now: Timestamp,
) -> Result<plugins::Generated> {
    check_content(&plugins::Source::parse(source)?.content())?;
    let now = Timestamp::from_second(now.as_second())?;
    let serial = match published {
        Some(bytes) => {
            let previous = plugins::Catalog::decode(bytes)?;
            check_content(&previous.content)?;
            ensure!(
                plugins::timestamp(&previous.generated_at).is_some_and(|at| at <= now.as_second()),
                "Preview generation clock precedes the verified published catalog"
            );
            previous
                .serial
                .checked_add(1)
                .context("Preview serial overflow")?
        }
        None => 1,
    };
    let expires = now.checked_add(SignedDuration::from_secs(EXPIRY_SECONDS))?;
    let generated = plugins::generate(source, verified, serial, now, expires)?;
    check(&generated.bytes, now)?;
    Ok(generated)
}

pub fn check(bytes: &[u8], now: Timestamp) -> Result<plugins::Catalog> {
    let catalog = plugins::Catalog::decode(bytes)?;
    check_content(&catalog.content)?;
    ensure!(
        plugins::timestamp(&catalog.expires_at).is_some_and(|at| at > now.as_second()),
        "Preview catalog has expired"
    );
    ensure!(
        plugins::timestamp(&catalog.generated_at).is_some_and(|at| at <= now.as_second()),
        "Preview catalog generation is in the future"
    );
    Ok(catalog)
}

/// Exact Statement/v1 and current cosign predicate, not a signature.
pub fn statement(bytes: &[u8]) -> Value {
    json!({"_type":"https://in-toto.io/Statement/v1",
        "subject":[{"name":CATALOG,"digest":{"sha256":hex::encode(Sha256::digest(bytes))}}],
        "predicateType":"https://sigstore.dev/cosign/sign/v1","predicate":{}})
}

/// Check the complete fixed statement shape, then reuse the ordinary bundle
/// envelope/media/digest check. Neither function verifies cryptography.
fn check_bundle(bundle: &[u8], bytes: &[u8]) -> Result<()> {
    release::check_bundle_for(bundle, bytes, CATALOG)?;
    let value: Value = serde_json::from_slice(bundle)?;
    let payload = base64::engine::general_purpose::STANDARD.decode(
        value["dsseEnvelope"]["payload"]
            .as_str()
            .unwrap_or_default(),
    )?;
    let actual: Value = serde_json::from_slice(&payload)?;
    ensure!(
        actual == statement(bytes),
        "Preview bundle must contain the exact Statement/v1 subject and cosign predicate"
    );
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub repository: &'static str,
    pub tag: &'static str,
    pub prerelease: bool,
    pub latest: bool,
    pub serial: u64,
    pub expires: String,
    pub assets: Vec<release::Asset>,
    pub cryptographic_verification: &'static str,
}

/// Plan exactly the two existing metadata assets. The operator independently
/// verifies their human signature before upload; a plan is never an approval.
pub fn plan(dir: &Path, now: Timestamp) -> Result<Plan> {
    let bytes = std::fs::read(dir.join(CATALOG))?;
    let bundle = std::fs::read(dir.join(BUNDLE))?;
    pair(&bytes, &bundle, now)
}

fn pair(bytes: &[u8], bundle: &[u8], now: Timestamp) -> Result<Plan> {
    let catalog = check(bytes, now)?;
    check_bundle(bundle, bytes)?;
    Ok(Plan { repository:"oyzuai/tool-index", tag:TAG, prerelease:true, latest:false,
        serial:catalog.serial, expires:catalog.expires_at,
        assets:vec![release::Asset { name:CATALOG.into(), bytes:bytes.len() as u64 },
            release::Asset { name:BUNDLE.into(), bytes:bundle.len() as u64 }],
        cryptographic_verification:"NOT performed by this helper; externally verify exact bytes with the approved human identity before publication" })
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
