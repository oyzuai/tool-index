//! Stamping the release copy, review ignoring the stamp, and the release
//! plan, on the sample index. No network and no signing: cosign signs and
//! verifies in the release workflow.
use base64::Engine as _;
use jiff::Timestamp;
use oyzu_tool_index::index::Index;
use oyzu_tool_index::release::{check_bundle, plan, BUNDLE, COMPRESSED, INDEX, NOTICE};
use oyzu_tool_index::review::{review, Decision};
use oyzu_tool_index::stamp::{stamp, stamp_after};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

fn sample() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-index.json"))
        .unwrap()
}

fn now() -> Timestamp {
    "2026-10-10T05:41:07.25Z".parse().unwrap()
}

fn doc(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn the_first_release_is_serial_one_and_expires_in_fourteen_days() {
    let out = stamp(&sample(), None, now(), 14).unwrap();
    let d = doc(&out);
    assert_eq!(d["serial"], 1);
    assert_eq!(d["expires"], "2026-10-24T05:41:07Z");
    // Everything else is the committed document, unchanged.
    let mut rest = d.clone();
    let obj = rest.as_object_mut().unwrap();
    obj.remove("serial");
    obj.remove("expires");
    assert_eq!(rest, doc(&sample()));
    assert!(out.ends_with(b"}\n"));
    // Stamping is a pure function of its inputs.
    assert_eq!(out, stamp(&sample(), None, now(), 14).unwrap());
}

#[test]
fn a_later_release_takes_the_published_serial_plus_one() {
    let first = stamp(&sample(), None, now(), 14).unwrap();
    let later: Timestamp = "2026-10-11T05:41:00Z".parse().unwrap();
    let second = stamp(&sample(), Some(&first), later, 14).unwrap();
    assert_eq!(doc(&second)["serial"], 2);
    assert_eq!(doc(&second)["expires"], "2026-10-25T05:41:00Z");
    let third = stamp(&sample(), Some(&second), later, 3).unwrap();
    assert_eq!(doc(&third)["serial"], 3);
    assert_eq!(doc(&third)["expires"], "2026-10-14T05:41:00Z");
}

#[test]
fn stamp_refuses_a_stamped_input_and_a_published_copy_without_serial() {
    let stamped = stamp(&sample(), None, now(), 14).unwrap();
    let err = stamp(&stamped, None, now(), 14).unwrap_err();
    assert!(format!("{err:#}").contains("already carries"), "{err:#}");

    let mut only_expires = doc(&sample());
    only_expires["expires"] = json!("2026-10-24T00:00:00Z");
    let only_expires = serde_json::to_vec(&only_expires).unwrap();
    assert!(stamp(&only_expires, None, now(), 14).is_err());

    let err = stamp(&sample(), Some(&sample()), now(), 14).unwrap_err();
    assert!(format!("{err:#}").contains("no serial"), "{err:#}");
    assert!(stamp(&sample(), None, now(), 0).is_err());
    assert!(stamp(b"{}", None, now(), 14).is_err());
}

#[test]
fn review_ignores_serial_and_expires() {
    let committed = Index::read(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-index.json"),
    )
    .unwrap();
    assert!(!committed.stamped());
    let one: Index = serde_json::from_slice(&stamp(&sample(), None, now(), 14).unwrap()).unwrap();
    let two: Index = serde_json::from_slice(
        &stamp(
            &sample(),
            Some(&one.to_bytes().unwrap()),
            "2026-10-11T00:00:00Z".parse().unwrap(),
            14,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(review(Some(&one), &two).decision, Decision::NoChange);
    assert_eq!(review(Some(&one), &committed).decision, Decision::NoChange);
    assert_eq!(review(Some(&committed), &two).decision, Decision::NoChange);
    // The unstamped model writes neither field.
    let bytes = committed.to_bytes().unwrap();
    assert!(doc(&bytes).get("serial").is_none() && doc(&bytes).get("expires").is_none());
}

fn zstd(bytes: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(bytes, ruzstd::encoding::CompressionLevel::Fastest)
}

fn bundle_for(index: &[u8]) -> Vec<u8> {
    let statement = json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{"name": "index.json", "digest": {"sha256": hex::encode(Sha256::digest(index))}}],
        "predicateType": "https://sigstore.dev/cosign/sign/v1",
        "predicate": {}
    });
    let payload =
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&statement).unwrap());
    serde_json::to_vec(&json!({
        "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
        "verificationMaterial": {},
        "dsseEnvelope": {
            "payload": payload,
            "payloadType": "application/vnd.in-toto+json",
            "signatures": [{"sig": ""}]
        }
    }))
    .unwrap()
}

#[test]
fn release_lists_the_four_assets_for_the_serial_tag() {
    let dir = tempfile::tempdir().unwrap();
    let index = stamp(&sample(), None, now(), 14).unwrap();
    std::fs::write(dir.path().join(INDEX), &index).unwrap();
    std::fs::write(dir.path().join(NOTICE), b"license\n").unwrap();

    // Before compressing and signing, a dry run reports both missing; a real run refuses.
    let dry = plan(dir.path(), now(), true).unwrap();
    assert_eq!(dry.tag, "index-v1");
    assert_eq!(
        dry.missing,
        vec![COMPRESSED.to_string(), BUNDLE.to_string()]
    );
    let err = plan(dir.path(), now(), false).unwrap_err();
    assert!(format!("{err:#}").contains(BUNDLE), "{err:#}");

    std::fs::write(dir.path().join(COMPRESSED), zstd(&index)).unwrap();
    std::fs::write(dir.path().join(BUNDLE), bundle_for(&index)).unwrap();
    let p = plan(dir.path(), now(), false).unwrap();
    assert_eq!(p.serial, 1);
    assert!(p.missing.is_empty());
    let names: Vec<&str> = p.assets.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, [INDEX, COMPRESSED, BUNDLE, NOTICE]);
    assert!(p.command.starts_with("gh release create index-v1 --latest"));
}

#[test]
fn release_refuses_unstamped_expired_and_mismatched_copies() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(NOTICE), b"license\n").unwrap();

    std::fs::write(dir.path().join(INDEX), sample()).unwrap();
    let err = plan(dir.path(), now(), true).unwrap_err();
    assert!(format!("{err:#}").contains("not stamped"), "{err:#}");

    let index = stamp(&sample(), None, now(), 14).unwrap();
    std::fs::write(dir.path().join(INDEX), &index).unwrap();
    let late: Timestamp = "2026-10-24T05:41:07Z".parse().unwrap();
    let err = plan(dir.path(), late, true).unwrap_err();
    assert!(format!("{err:#}").contains("expired"), "{err:#}");

    // A compressed copy of other bytes, or not zstd at all, is refused.
    std::fs::write(dir.path().join(COMPRESSED), zstd(&sample())).unwrap();
    let err = plan(dir.path(), now(), true).unwrap_err();
    assert!(
        format!("{err:#}").contains("does not decompress to index.json"),
        "{err:#}"
    );
    std::fs::write(dir.path().join(COMPRESSED), b"plain").unwrap();
    let err = plan(dir.path(), now(), true).unwrap_err();
    assert!(format!("{err:#}").contains("not zstd"), "{err:#}");
    std::fs::write(dir.path().join(COMPRESSED), zstd(&index)).unwrap();

    // A bundle for other bytes (here the unstamped copy) is refused.
    std::fs::write(dir.path().join(BUNDLE), bundle_for(&sample())).unwrap();
    let err = plan(dir.path(), now(), true).unwrap_err();
    assert!(format!("{err:#}").contains("does not sign"), "{err:#}");

    assert!(check_bundle(
        b"{\"mediaType\":\"application/vnd.dev.sigstore.bundle+json;version=0.2\"}",
        &index
    )
    .is_err());
}

/// The published copy must be the release with the highest `index-v` tag, so
/// a moved "latest" marker or a stray tag cannot reuse or skip a serial.
#[test]
fn stamp_follows_the_highest_release_tag() {
    let first = stamp(&sample(), None, now(), 14).unwrap();
    let second = stamp(&sample(), Some(&first), now(), 14).unwrap();
    // index-v2 is the highest tag and the copy given is serial 2.
    let third = stamp_after(&sample(), Some(&second), Some(2), now(), 14).unwrap();
    assert_eq!(doc(&third)["serial"], 3);
    // "latest" pointed at index-v1 while index-v2 exists.
    let err = stamp_after(&sample(), Some(&first), Some(2), now(), 14).unwrap_err();
    assert!(err.to_string().contains("index-v2"), "{err}");
    // A tag exists but no copy was downloaded: serial 1 would be reused.
    assert!(stamp_after(&sample(), None, Some(2), now(), 14).is_err());
    assert_eq!(
        doc(&stamp_after(&sample(), None, None, now(), 14).unwrap())["serial"],
        1
    );
}
