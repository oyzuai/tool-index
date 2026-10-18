//! The plugin catalog in the release plan: it joins the release only when it
//! belongs to it, and nothing about it can stop the tools release. Generation
//! and the contract are covered by the unit tests in `src/plugins_tests.rs`.
use base64::Engine as _;
use jiff::Timestamp;
use oyzu_tool_index::plugins::generate;
use oyzu_tool_index::release::{plan, BUNDLE, COMPRESSED, INDEX, NOTICE, PLUGINS, PLUGINS_BUNDLE};
use oyzu_tool_index::stamp::stamp;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

fn now() -> Timestamp {
    "2026-10-10T06:00:00Z".parse().unwrap()
}

fn bundle_for(name: &str, bytes: &[u8]) -> Vec<u8> {
    let statement = json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{"name": name, "digest": {"sha256": hex::encode(Sha256::digest(bytes))}}],
        "predicateType": "https://sigstore.dev/cosign/sign/v1",
        "predicate": {}
    });
    serde_json::to_vec(&json!({
        "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
        "dsseEnvelope": {
            "payload": base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&statement).unwrap()),
            "payloadType": "application/vnd.in-toto+json",
            "signatures": [{"sig": ""}]
        }
    }))
    .unwrap()
}

/// A complete tools release directory (serial 1) and its index bytes.
fn tools_release(dir: &Path) -> Vec<u8> {
    let sample = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-index.json"),
    )
    .unwrap();
    let index = stamp(&sample, None, now(), 14).unwrap();
    std::fs::write(dir.join(INDEX), &index).unwrap();
    std::fs::write(dir.join(NOTICE), b"license\n").unwrap();
    std::fs::write(
        dir.join(COMPRESSED),
        ruzstd::encoding::compress_to_vec(&index[..], ruzstd::encoding::CompressionLevel::Fastest),
    )
    .unwrap();
    std::fs::write(dir.join(BUNDLE), bundle_for(INDEX, &index)).unwrap();
    index
}

fn empty_catalog(serial: u64) -> Vec<u8> {
    let expires: Timestamp = "2026-10-24T06:00:00Z".parse().unwrap();
    generate("authority = \"oyzu\"\n", None, serial, now(), expires)
        .unwrap()
        .bytes
}

fn names(p: &oyzu_tool_index::release::Plan) -> Vec<&str> {
    p.assets.iter().map(|a| a.name.as_str()).collect()
}

#[test]
fn a_matching_signed_catalog_joins_the_release() {
    let dir = tempfile::tempdir().unwrap();
    tools_release(dir.path());
    let catalog = empty_catalog(1);
    std::fs::write(dir.path().join(PLUGINS), &catalog).unwrap();
    std::fs::write(
        dir.path().join(PLUGINS_BUNDLE),
        bundle_for(PLUGINS, &catalog),
    )
    .unwrap();
    let p = plan(dir.path(), now(), false).unwrap();
    assert!(p.plugins.published, "{:?}", p.plugins.skipped);
    assert_eq!(
        names(&p),
        [INDEX, COMPRESSED, BUNDLE, NOTICE, PLUGINS, PLUGINS_BUNDLE]
    );
    assert!(p.command.contains(PLUGINS_BUNDLE));
}

#[test]
fn nothing_about_the_catalog_can_stop_the_tools_release() {
    let dir = tempfile::tempdir().unwrap();
    let tools_only = [INDEX, COMPRESSED, BUNDLE, NOTICE];
    tools_release(dir.path());

    // No catalog at all.
    let p = plan(dir.path(), now(), false).unwrap();
    assert!(!p.plugins.published);
    assert_eq!(names(&p), tools_only);

    let good = empty_catalog(1);
    let cases: Vec<(&str, Vec<u8>, Vec<u8>)> = vec![
        // A catalog for another release's serial.
        (
            "serial",
            empty_catalog(2),
            bundle_for(PLUGINS, &empty_catalog(2)),
        ),
        // A bundle signing other bytes, or another subject name.
        ("bundle", good.clone(), bundle_for(PLUGINS, b"other")),
        ("name", good.clone(), bundle_for(INDEX, &good)),
        // Not a catalog.
        ("garbage", b"{".to_vec(), bundle_for(PLUGINS, b"{")),
        // A contract violation: digest no longer matches the content.
        (
            "digest",
            String::from_utf8(good.clone())
                .unwrap()
                .replace("\"authority\": \"oyzu\"", "\"authority\": \"acme\"")
                .into_bytes(),
            vec![],
        ),
    ];
    for (name, catalog, bundle) in cases {
        let bundle = if bundle.is_empty() {
            bundle_for(PLUGINS, &catalog)
        } else {
            bundle
        };
        std::fs::write(dir.path().join(PLUGINS), &catalog).unwrap();
        std::fs::write(dir.path().join(PLUGINS_BUNDLE), &bundle).unwrap();
        let p = plan(dir.path(), now(), false).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert!(!p.plugins.published, "{name}");
        assert!(p.plugins.skipped.is_some(), "{name}");
        assert_eq!(names(&p), tools_only, "{name}");
    }

    // An expired catalog (the index is still valid), and one without its bundle.
    let short: Timestamp = "2026-10-11T06:00:00Z".parse().unwrap();
    let expiring = generate("authority = \"oyzu\"\n", None, 1, now(), short)
        .unwrap()
        .bytes;
    std::fs::write(dir.path().join(PLUGINS), &expiring).unwrap();
    std::fs::write(
        dir.path().join(PLUGINS_BUNDLE),
        bundle_for(PLUGINS, &expiring),
    )
    .unwrap();
    let later: Timestamp = "2026-10-12T06:00:00Z".parse().unwrap();
    let p = plan(dir.path(), later, false).unwrap();
    assert!(!p.plugins.published && names(&p) == tools_only);
    assert!(p.plugins.skipped.unwrap().contains("expired"));
    std::fs::write(dir.path().join(PLUGINS), &good).unwrap();
    std::fs::write(dir.path().join(PLUGINS_BUNDLE), bundle_for(PLUGINS, &good)).unwrap();
    assert!(plan(dir.path(), now(), false).unwrap().plugins.published);
    std::fs::remove_file(dir.path().join(PLUGINS_BUNDLE)).unwrap();
    let p = plan(dir.path(), now(), false).unwrap();
    assert!(!p.plugins.published && names(&p) == tools_only);

    // A dry run with an unsigned catalog reports the bundle as missing.
    let p = plan(dir.path(), now(), true).unwrap();
    assert!(p.missing.contains(&PLUGINS_BUNDLE.to_string()));
    assert!(names(&p).contains(&PLUGINS));
}
