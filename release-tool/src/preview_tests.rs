//! Component fixtures are not real human signatures or public availability.
use super::*;

fn now() -> Timestamp {
    "2026-10-24T12:00:00Z".parse().unwrap()
}
fn empty() -> &'static str {
    "authority = \"oyzu\"\n"
}

fn bundle(statement: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"mediaType":release::BUNDLE_MEDIA_TYPE,
        "dsseEnvelope":{"payloadType":release::PAYLOAD_TYPE,
            "payload":base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&statement).unwrap())}})).unwrap()
}

fn proof(authority: &str, package: &str, version: &str) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"schemaVersion":2,"mediaType":INDEX,"artifactType":RELEASE,"manifests":[],
        "annotations":{"dev.oyzu.plugin.authority":authority,
            "org.opencontainers.image.title":package,"org.opencontainers.image.version":version}}),
    )
    .unwrap()
}

fn withdrawal(bytes: &[u8]) -> plugins::Withdrawal {
    serde_json::from_value(
        json!({"package":"oyzu/go","version":"0.1.0","root_digest":sha(bytes),
        "root_preimage":base64::engine::general_purpose::STANDARD.encode(bytes),
        "kind":"revoked","at":"2026-10-24T11:00:00Z","reason":"test-only exact root withdrawal"}),
    )
    .unwrap()
}

#[test]
fn standalone_generate_check_statement_and_pair_do_not_need_a_tool_index() {
    let first = generate(empty(), None, None, now()).unwrap();
    assert_eq!(first.catalog.serial, 1);
    assert_eq!(first.catalog.generated_at, "2026-10-24T12:00:00Z");
    assert_eq!(first.catalog.expires_at, "2026-11-07T12:00:00Z");
    check(&first.bytes, now()).unwrap();
    let statement = statement(&first.bytes);
    assert_eq!(statement["subject"][0]["name"], CATALOG);
    assert_ne!(statement["subject"][0]["name"], release::PLUGINS);
    let plan = pair(&first.bytes, &bundle(statement), now()).unwrap();
    assert_eq!(plan.tag, "plugins-preview");
    assert!(plan.prerelease && !plan.latest);
    assert_eq!(
        plan.assets
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        vec![CATALOG, BUNDLE]
    );
    assert!(plan.cryptographic_verification.contains("NOT performed"));
}

#[test]
fn serial_succession_uses_only_the_supplied_verified_preview_document() {
    let first = generate(empty(), None, None, now()).unwrap();
    let next = generate(empty(), None, Some(&first.bytes), now()).unwrap();
    assert_eq!(next.catalog.serial, 2);
    assert_eq!(next.catalog.content_digest, first.catalog.content_digest);
    assert!(generate(empty(), None, Some(b"{}"), now()).is_err());
    let mut future = first.catalog;
    future.generated_at = "2026-10-25T12:00:00Z".into();
    assert!(generate(
        empty(),
        None,
        Some(&serde_json::to_vec(&future).unwrap()),
        now()
    )
    .is_err());
}

#[test]
fn expiry_is_current_and_never_exceeds_the_existing_wire_cap() {
    let first = generate(empty(), None, None, now()).unwrap();
    assert!(check(&first.bytes, "2026-11-07T12:00:00Z".parse().unwrap()).is_err());
    assert!(check(&first.bytes, "2026-10-23T12:00:00Z".parse().unwrap()).is_err());
    let mut too_long = first.catalog;
    too_long.expires_at = "2026-11-25T12:00:00Z".into();
    assert!(check(&serde_json::to_vec(&too_long).unwrap(), now()).is_err());
}

#[test]
fn preview_scope_is_checked_before_unavailable_releases_are_omitted() {
    let mut content: plugins::Catalog = serde_json::from_str(include_str!(
        "../../tests/fixtures/plugins-v1alpha1/catalog.go-preview.json"
    ))
    .unwrap();
    content.content.withdrawals.clear();
    check_content(&content.content).unwrap();
    let source = toml::to_string(&content.content).unwrap();
    assert!(generate(&source, None, None, now())
        .unwrap()
        .catalog
        .content
        .packages
        .is_empty());
    content.content.packages[0].releases[0].preview = false;
    assert!(generate(
        &toml::to_string(&content.content).unwrap(),
        None,
        None,
        now()
    )
    .is_err());
    content.content.packages[0].releases[0].preview = true;
    let mut other = content.content.clone();
    other.packages[0].publisher = "acme".into();
    other.packages[0].releases[0].routes = vec![plugins::Route::Https(
        "https://packages.example.test/go.tar.gz".into(),
    )];
    other.references.clear();
    other.detections.clear();
    other.check().unwrap();
    assert!(check_content(&other).is_err());
    content.content.authority = "other".into();
    assert!(check_content(&content.content).is_err());
}

#[test]
fn local_withdrawals_prove_exact_raw_bytes_class_publisher_and_tuple() {
    let mut content = plugins::Source::parse(empty()).unwrap().content();
    let original = proof("local-preview", "oyzu/go", "0.1.0");
    content.withdrawals = vec![withdrawal(&original)];
    check_content(&content).unwrap();
    let source = toml::to_string(&content).unwrap();
    let generated = generate(&source, None, None, now()).unwrap();
    assert_eq!(generated.catalog.content.withdrawals, content.withdrawals);
    let original_digest = generated.catalog.content_digest;
    let mut padded = original.clone();
    padded.push(b'\n');
    content.withdrawals[0].root_preimage =
        Some(base64::engine::general_purpose::STANDARD.encode(&padded));
    assert!(check_content(&content).is_err()); // No reserialization before hashing.
    content.withdrawals[0] = withdrawal(&padded);
    check_content(&content).unwrap();
    assert_ne!(plugins::content_digest(&content).unwrap(), original_digest);
    for bad in [
        proof("workflow", "oyzu/go", "0.1.0"),
        proof("local-preview", "oyzu/go", "0.2.0"),
    ] {
        content.withdrawals[0] = withdrawal(&bad);
        assert!(check_content(&content).is_err());
    }
    content.withdrawals[0] = withdrawal(&proof("local-preview", "acme/go", "0.1.0"));
    content.withdrawals[0].package = "acme/go".into();
    assert!(check_content(&content).is_err());
    content.withdrawals[0] = withdrawal(&original);
    content.withdrawals[0].root_preimage = None;
    assert!(check_content(&content).is_err());
}

#[test]
fn fixed_pair_refuses_wrong_subject_predicate_or_catalog_bytes() {
    let generated = generate(empty(), None, None, now()).unwrap();
    let original = statement(&generated.bytes);
    let mut primary = original.clone();
    primary["subject"][0]["name"] = release::PLUGINS.into();
    assert!(pair(&generated.bytes, &bundle(primary), now()).is_err());
    let mut old_statement = original.clone();
    old_statement["_type"] = "https://in-toto.io/Statement/v0.1".into();
    assert!(pair(&generated.bytes, &bundle(old_statement), now()).is_err());
    let mut predicate = original.clone();
    predicate["predicateType"] = "https://example.test/other".into();
    assert!(pair(&generated.bytes, &bundle(predicate), now()).is_err());
    let mut modified = generated.bytes;
    modified.push(b'\n');
    assert!(pair(&modified, &bundle(original), now()).is_err());
}
