//! The port against the canonical oyzu fixtures (vendored under
//! `tests/fixtures/plugins-v1alpha1`, pinned in its README), and the
//! generator's own behavior.
use super::*;
use serde_json::json;

const PREVIEW: &str = include_str!("../tests/fixtures/plugins-v1alpha1/catalog.go-preview.json");
const MAJORS: &str = include_str!("../tests/fixtures/plugins-v1alpha1/catalog.go-majors.json");
const VALID: &str = include_str!("../tests/fixtures/plugins-v1alpha1/valid-cases.json");
const INVALID: &str = include_str!("../tests/fixtures/plugins-v1alpha1/invalid-cases.json");

fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn now() -> Timestamp {
    at("2026-10-19T06:00:00Z")
}

fn expires() -> Timestamp {
    at("2026-11-02T06:00:00Z")
}

/// The example's `content` as `config/plugins.toml` text.
fn source_of(example: &str) -> String {
    let catalog: Catalog = serde_json::from_str(example).unwrap();
    toml::to_string(&catalog.content).unwrap()
}

fn verified_all(example: &str) -> Vec<u8> {
    let catalog: Catalog = serde_json::from_str(example).unwrap();
    let releases: Vec<_> = candidates(&catalog.content)
        .into_iter()
        .map(|c| json!({"package": c.package, "version": c.version, "root_digest": c.root_digest}))
        .collect();
    serde_json::to_vec(&json!({ "releases": releases })).unwrap()
}

#[test]
fn canonical_examples_decode_and_their_content_digests_match() {
    for (name, text) in [("go-preview", PREVIEW), ("go-majors", MAJORS)] {
        let catalog = Catalog::decode(text.as_bytes()).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            content_digest(&catalog.content).unwrap(),
            value["content_digest"].as_str().unwrap(),
            "{name}"
        );
        // The typed model re-encodes to exactly the example's JSON.
        assert_eq!(serde_json::to_value(&catalog).unwrap(), value, "{name}");
    }
}

fn pointer_parent<'a>(doc: &'a mut Value, pointer: &str) -> (&'a mut Value, String) {
    let (parent, last) = pointer.rsplit_once('/').unwrap();
    (doc.pointer_mut(parent).unwrap(), last.to_owned())
}

fn mutate(doc: &mut Value, case: &Value) {
    let pointer = case["pointer"].as_str().unwrap();
    let (parent, last) = pointer_parent(doc, pointer);
    if case["remove"] == true {
        match parent {
            Value::Object(m) => {
                m.remove(&last).unwrap();
            }
            Value::Array(a) => {
                a.remove(last.parse::<usize>().unwrap());
            }
            _ => panic!("cannot remove"),
        }
        return;
    }
    let mut value = case["value"].clone();
    if let Some(n) = case.get("repeat").and_then(Value::as_u64) {
        value = Value::Array(vec![value; n as usize]);
    }
    match parent {
        Value::Object(m) => {
            m.insert(last, value);
        }
        Value::Array(a) => {
            let i = last.parse::<usize>().unwrap();
            if i == a.len() {
                a.push(value);
            } else {
                a[i] = value;
            }
        }
        _ => panic!("cannot set"),
    }
}

#[test]
fn every_canonical_invalid_catalog_case_is_refused() {
    let cases: Vec<Value> = serde_json::from_str(INVALID).unwrap();
    let mut ran = 0;
    for case in cases {
        let base = case["base"].as_str().unwrap();
        if !base.starts_with("catalog.") {
            continue;
        }
        let mut doc: Value = serde_json::from_str(match base {
            "catalog.go-preview.json" => PREVIEW,
            "catalog.go-majors.json" => MAJORS,
            other => panic!("unknown base {other}"),
        })
        .unwrap();
        mutate(&mut doc, &case);
        restamp(&mut doc, &case);
        let bytes = serde_json::to_vec(&doc).unwrap();
        assert!(
            Catalog::decode(&bytes).is_err(),
            "accepted: {}",
            case["name"]
        );
        ran += 1;
    }
    assert!(ran >= 100, "only {ran} catalog cases ran");
}

/// Mutations under `/content` are re-stamped with the digest of the mutated
/// content, so the case breaks the rule it names and not the digest.
fn restamp(doc: &mut Value, case: &Value) {
    let pointer = case["pointer"].as_str().unwrap();
    if pointer == "/content" || pointer.starts_with("/content/") {
        doc["content_digest"] = json!(canonical_digest(&doc["content"]).unwrap());
    }
}

#[test]
fn every_canonical_valid_catalog_mutation_is_accepted() {
    let cases: Vec<Value> = serde_json::from_str(VALID).unwrap();
    let mut ran = 0;
    for case in cases {
        let base = case["base"].as_str().unwrap();
        if !base.starts_with("catalog.") {
            continue;
        }
        let mut doc: Value = serde_json::from_str(match base {
            "catalog.go-preview.json" => PREVIEW,
            "catalog.go-majors.json" => MAJORS,
            other => panic!("unknown base {other}"),
        })
        .unwrap();
        mutate(&mut doc, &case);
        restamp(&mut doc, &case);
        let bytes = serde_json::to_vec(&doc).unwrap();
        Catalog::decode(&bytes).unwrap_or_else(|e| panic!("{}: {e:#}", case["name"]));
        ran += 1;
    }
    assert!(ran >= 8, "only {ran} catalog cases ran");
}

#[test]
fn a_fixture_entry_round_trips_through_the_source_and_keeps_its_digest() {
    for example in [PREVIEW, MAJORS] {
        let expected: Catalog = serde_json::from_str(example).unwrap();
        let source = source_of(example);
        let verified = verified_all(example);
        let out = generate(&source, Some(&verified), expected.serial, now(), expires()).unwrap();
        assert!(out.omitted.is_empty());
        assert_eq!(out.catalog.content, expected.content);
        assert_eq!(out.catalog.content_digest, expected.content_digest);
        // Decoding the written bytes gives the same document back.
        assert_eq!(Catalog::decode(&out.bytes).unwrap(), out.catalog);
    }
}

#[test]
fn the_empty_catalog_is_well_formed() {
    let out = generate("authority = \"oyzu\"\n", None, 7, now(), expires()).unwrap();
    let doc: Value = serde_json::from_slice(&out.bytes).unwrap();
    assert_eq!(doc["serial"], 7);
    assert_eq!(doc["generated_at"], "2026-10-19T06:00:00Z");
    assert_eq!(doc["expires_at"], "2026-11-02T06:00:00Z");
    assert_eq!(
        doc["content"],
        json!({"authority": "oyzu", "packages": [], "references": [], "detections": [], "withdrawals": []})
    );
    assert!(out.bytes.ends_with(b"}\n"));
    Catalog::decode(&out.bytes).unwrap();
}

#[test]
fn expiry_is_capped_at_the_clients_31_days() {
    let far = at("2027-10-19T06:00:00Z");
    let out = generate("authority = \"oyzu\"\n", None, 1, now(), far).unwrap();
    assert_eq!(out.catalog.expires_at, "2026-11-19T06:00:00Z");
    // A fractional-second clock is truncated like the index's stamp.
    let out = generate(
        "authority = \"oyzu\"\n",
        None,
        1,
        at("2026-10-19T06:00:00.9Z"),
        far,
    )
    .unwrap();
    assert_eq!(out.catalog.generated_at, "2026-10-19T06:00:00Z");
}

#[test]
fn a_daily_re_sign_keeps_the_content_digest() {
    let source = source_of(MAJORS);
    let verified = verified_all(MAJORS);
    let a = generate(&source, Some(&verified), 10, now(), expires()).unwrap();
    let b = generate(
        &source,
        Some(&verified),
        11,
        at("2026-10-20T06:00:00Z"),
        at("2026-11-03T06:00:00Z"),
    )
    .unwrap();
    assert_eq!(a.catalog.content_digest, b.catalog.content_digest);
    assert_ne!(a.bytes, b.bytes);
    // Same inputs, same bytes.
    assert_eq!(
        a.bytes,
        generate(&source, Some(&verified), 10, now(), expires())
            .unwrap()
            .bytes
    );
}

#[test]
fn only_verified_releases_are_advertised() {
    let source = source_of(MAJORS);
    // Nothing verified, or no verification at all: no packages, and the
    // references and detections that depended on them go too.
    for verified in [None, Some(br#"{"releases":[]}"#.as_slice())] {
        let out = generate(&source, verified, 1, now(), expires()).unwrap();
        assert!(out.catalog.content.packages.is_empty());
        assert!(out.catalog.content.references.is_empty());
        assert!(out.catalog.content.detections.is_empty());
        assert!(!out.omitted.is_empty());
        // The withdrawal stays and loses its dropped advisory replacement.
        assert_eq!(out.catalog.content.withdrawals.len(), 1);
        assert_eq!(out.catalog.content.withdrawals[0].replacement, None);
    }
    // One release verified: only it is listed, and the document still passes.
    let all: Catalog = serde_json::from_str(MAJORS).unwrap();
    let release = &all.content.packages[0].releases[1];
    let one = serde_json::to_vec(&json!({"releases": [{
        "package": "oyzu/go", "version": release.version, "root_digest": release.root_digest
    }]}))
    .unwrap();
    let out = generate(&source, Some(&one), 1, now(), expires()).unwrap();
    let versions: Vec<_> = out.catalog.content.packages[0]
        .releases
        .iter()
        .map(|r| r.version.as_str())
        .collect();
    assert_eq!(versions, [release.version.as_str()]);
    assert_eq!(
        out.omitted.len(),
        all.content.packages[0].releases.len() - 1
    );
    // A digest that differs from the source is not the release.
    let wrong = serde_json::to_vec(&json!({"releases": [{
        "package": "oyzu/go", "version": release.version,
        "root_digest": format!("sha256:{}", "0".repeat(64))
    }]}))
    .unwrap();
    let out = generate(&source, Some(&wrong), 1, now(), expires()).unwrap();
    assert!(out.catalog.content.packages.is_empty());
}

#[test]
fn a_broken_source_fails_generation_with_no_output() {
    // Unknown field, unsorted lists and an unlisted reference all fail.
    assert!(generate(
        "authority = \"oyzu\"\nmirrors = []\n",
        None,
        1,
        now(),
        expires()
    )
    .is_err());
    assert!(generate("authority = \"Oyzu\"\n", None, 1, now(), expires()).is_err());
    assert!(generate("not toml [", None, 1, now(), expires()).is_err());
    let mut catalog: Catalog = serde_json::from_str(PREVIEW).unwrap();
    catalog.content.references.reverse();
    let source = toml::to_string(&catalog.content).unwrap();
    let err = generate(&source, None, 1, now(), expires()).unwrap_err();
    assert!(format!("{err:#}").contains("sorted and unique"), "{err:#}");
    // A malformed availability file is an error, not an empty list.
    assert!(generate("authority = \"oyzu\"\n", Some(b"[]"), 1, now(), expires()).is_err());
}

#[test]
fn the_release_check_ties_the_catalog_to_the_index() {
    let out = generate("authority = \"oyzu\"\n", None, 5, now(), expires()).unwrap();
    check_release(&out.bytes, 5, now()).unwrap();
    let err = check_release(&out.bytes, 6, now()).unwrap_err();
    assert!(err.to_string().contains("serial"), "{err}");
    let err = check_release(&out.bytes, 5, at("2026-11-02T06:00:00Z")).unwrap_err();
    assert!(err.to_string().contains("expired"), "{err}");
    assert!(check_release(&vec![b' '; MAX_CATALOG_BYTES + 1], 5, now()).is_err());
}

#[test]
fn one_root_digest_names_one_release_of_a_package() {
    let mut catalog: Catalog = serde_json::from_str(MAJORS).unwrap();
    let first = catalog.content.packages[0].releases[0].clone();
    let second = &mut catalog.content.packages[0].releases[1];
    second.root_digest = first.root_digest.clone();
    second.routes = first.routes.clone();
    catalog.content_digest = content_digest(&catalog.content).unwrap();
    let err = Catalog::decode(&serde_json::to_vec(&catalog).unwrap()).unwrap_err();
    assert!(
        format!("{err:#}").contains("more than one release"),
        "{err:#}"
    );
}

#[test]
fn review_surfaces_capability_sensitive_changes() {
    let base = source_of(PREVIEW);
    // Unchanged: nothing to review.
    assert!(review(Some(&base), &base).unwrap().is_empty());

    // Broader detector reach: a changed matcher is sensitive.
    let mut catalog: Catalog = serde_json::from_str(PREVIEW).unwrap();
    catalog.content.detections[1].matcher = Matcher::File("go.mod".into());
    let changes = review(Some(&base), &toml::to_string(&catalog.content).unwrap()).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (changes[0].section, changes[0].kind, changes[0].sensitive),
        ("detections", "changed", true)
    );
    assert_eq!(changes[0].fields, ["match"]);

    // A new requirement (core feature) and a changed route are sensitive; a
    // support message alone is not.
    let mut catalog: Catalog = serde_json::from_str(PREVIEW).unwrap();
    catalog.content.packages[0].releases[0].roles[0]
        .features
        .push("zzz-new".into());
    let changes = review(Some(&base), &toml::to_string(&catalog.content).unwrap()).unwrap();
    assert!(
        changes[0].sensitive && changes[0].fields == ["roles"],
        "{changes:?}"
    );

    let mut catalog: Catalog = serde_json::from_str(PREVIEW).unwrap();
    catalog.content.packages[0].releases[0]
        .qualification
        .support
        .message = Some("note".into());
    let changes = review(Some(&base), &toml::to_string(&catalog.content).unwrap()).unwrap();
    assert_eq!(changes.len(), 1);
    assert!(!changes[0].sensitive, "{changes:?}");

    // A changed maintainer is sensitive; the first source ever is all "added".
    let mut catalog: Catalog = serde_json::from_str(PREVIEW).unwrap();
    catalog.content.packages[0].maintainer = "Someone else".into();
    let changes = review(Some(&base), &toml::to_string(&catalog.content).unwrap()).unwrap();
    assert!(changes[0].sensitive && changes[0].section == "packages");
    let first = review(None, &base).unwrap();
    assert!(first.iter().all(|c| c.kind == "added") && first.iter().any(|c| c.sensitive));
}

#[test]
fn the_committed_example_source_matches_the_canonical_catalog() {
    let text = include_str!("../tests/fixtures/plugins-v1alpha1/plugins.go-preview.toml");
    let canonical: Catalog = serde_json::from_str(PREVIEW).unwrap();
    assert_eq!(Source::parse(text).unwrap().content(), canonical.content);
}
