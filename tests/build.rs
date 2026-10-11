//! The build and review steps on a two-package snapshot, with a stand-in
//! `oyzu` that emits fixed records, so no network is needed.
use jiff::Timestamp;
use oyzu_tool_index::attempts::Attempts;
use oyzu_tool_index::build::{build, Inputs, Report};
use oyzu_tool_index::config::Config;
use oyzu_tool_index::index::{verified, DateSource, Index, ServingClass};
use oyzu_tool_index::resolve::Oyzu;
use oyzu_tool_index::review::{review, Decision};
use oyzu_tool_index::versions::{Listed, Listing, ToolListing};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const REGISTRY: &str = r#"packages:
  - type: github_release
    repo_owner: acme
    repo_name: tool
    asset: tool_{{trimV .Version}}_{{.OS}}_{{.Arch}}.tar.gz
    version_filter: not (Version matches "-nightly")
    checksum:
      type: github_release
      asset: checksums.txt
      algorithm: sha256
  - type: github_release
    repo_owner: acme
    repo_name: other
    asset: other_{{.OS}}_{{.Arch}}.zip
"#;

fn names_crate() -> Vec<u8> {
    let mut tar_bytes = Vec::new();
    {
        let mut b = tar::Builder::new(&mut tar_bytes);
        {
            let (name, text) = (
                "acme-tool",
                "aliases = [\"tool\"]\nbackends = [\"aqua:acme/tool\"]\n",
            );
            let mut h = tar::Header::new_gnu();
            h.set_size(text.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(
                &mut h,
                format!("mise-1.0/registry/{name}.toml"),
                text.as_bytes(),
            )
            .unwrap();
        }
        b.finish().unwrap();
    }
    let mut out = Vec::new();
    let mut e = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
    std::io::Write::write_all(&mut e, &tar_bytes).unwrap();
    e.finish().unwrap();
    out
}

fn digest(b: &[u8]) -> String {
    oyzu_tool_index::registry::digest(b)
}

/// A stand-in for `oyzu --json tools resolve|evidence`: resolve prints one
/// record per requested version; evidence marks every record verified.
fn fake_oyzu(dir: &Path) -> PathBuf {
    let path = dir.join("oyzu");
    let script = r#"#!/bin/sh
# usage: oyzu --json tools resolve ... | oyzu --json tools evidence --set FILE
#        oyzu --json tools evidence --provider NAME --version V... --platform P...
if [ "$3" = "evidence" ] && [ "$4" = "--provider" ]; then
  versions=""
  platforms=""
  prev=""
  for a in "$@"; do
    if [ "$prev" = "--version" ]; then versions="$versions $a"; fi
    if [ "$prev" = "--platform" ]; then platforms="$platforms $a"; fi
    prev="$a"
  done
  sets=""
  for v in $versions; do
    for p in $platforms; do
      r="{\"tool\":{\"id\":\"core:$5\"},\"version\":{\"requested\":\"$v\",\"normalized\":\"$v\"},\"platform\":\"$p\",\"status\":\"resolved\",\"tier\":\"verified\",\"verified\":{\"status\":\"ok\"},\"urls\":[{\"role\":\"artifact\",\"url\":\"https://github.com/adoptium/temurin-binaries/releases/download/$v/jdk.tar.gz\"}]}"
      if [ -z "$sets" ]; then sets="$r"; else sets="$sets,$r"; fi
    done
  done
  printf '{"format":1,"kind":"artifact-sets","engine":{"name":"oyzu","version":"test","evaluator_digest":"sha256:ee"},"sets":[%s]}' "$sets"
  exit 0
fi
if [ "$3" = "evidence" ]; then
  # A `fail` file beside this script keeps every record at the discovery tier.
  if [ -f "$(dirname "$0")/fail" ]; then cat "$5"; exit 3; fi
  sed 's/"tier":"discovery"/"tier":"verified","verified":{"status":"ok"}/g' "$5"
  exit 0
fi
sets=""
prev=""
tool=""
for a in "$@"; do
  if [ "$prev" = "--tool" ]; then tool="$a"; fi
  if [ "$prev" = "--version" ]; then
    n="${a#v}"
    r="{\"tool\":{\"id\":\"aqua:$tool\"},\"version\":{\"requested\":\"$a\",\"normalized\":\"$n\"},\"platform\":\"linux/amd64/gnu\",\"status\":\"resolved\",\"tier\":\"discovery\",\"urls\":[{\"role\":\"artifact\",\"url\":\"https://github.com/acme/tool/releases/download/$a/t.tar.gz\"}]}"
    if [ -z "$sets" ]; then sets="$r"; else sets="$sets,$r"; fi
  fi
  prev="$a"
done
printf '{"format":1,"kind":"artifact-sets","engine":{"name":"oyzu","version":"test","evaluator_digest":"sha256:ee"},"sets":[%s]}' "$sets"
"#;
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: Config,
}

fn fixture(reviewed: bool) -> Fixture {
    fixture_with(reviewed, "", "")
}

/// [`fixture`] with extra `index.toml` and `serving.toml` text.
fn fixture_with(reviewed: bool, index_extra: &str, serving_extra: &str) -> Fixture {
    fixture_full(
        reviewed,
        index_extra,
        serving_extra,
        "[[tool]]\npackage = \"acme/tool\"\nname = \"tool\"\ngroup = \"Test\"\n",
        10,
    )
}

fn fixture_full(
    reviewed: bool,
    index_extra: &str,
    serving_extra: &str,
    launch_set: &str,
    budget: usize,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    std::fs::write(root.join("registry.yaml"), REGISTRY).unwrap();
    let names = names_crate();
    let cfg = root.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("index.toml"),
        format!(
            r#"platforms = ["linux/amd64/gnu"]
[aqua_registry]
source = "test/registry"
tag = "v1"
digest = "{}"
registry_url = "file:///unused"
license_url = "file:///unused"
[names]
source = "mise"
version = "1.0"
digest = "{}"
url = "file:///unused"
[discovery]
max_versions = 3
source_kinds = ["github_release"]
[verified]
backfill = 2
max_versions_per_run = {budget}
keep_records = 2
include_prereleases = false
retry_after_days = 7
[signing]
expires_days = 14
{index_extra}"#,
            digest(REGISTRY.as_bytes()),
            digest(&names)
        ),
    )
    .unwrap();
    std::fs::write(cfg.join("launch-set.toml"), launch_set).unwrap();
    std::fs::write(
        cfg.join("serving.toml"),
        format!(
            "[tools.\"aqua:acme/tool\"]\nlicense = \"MIT\"\nreviewed = {reviewed}\n{serving_extra}"
        ),
    )
    .unwrap();
    let config = Config::load(&cfg).unwrap();
    Fixture {
        _dir: dir,
        root,
        config,
    }
}

fn listed(v: &str, date: Option<&str>) -> Listed {
    Listed {
        version: v.into(),
        published_at: date.map(str::to_owned),
        prerelease: v.contains("-rc"),
    }
}

fn listing(tool: &[Listed], other: &[Listed]) -> Listing {
    let mut tools = BTreeMap::new();
    tools.insert(
        "aqua:acme/tool".into(),
        ToolListing {
            source: "test".into(),
            versions: tool.to_vec(),
            error: None,
        },
    );
    tools.insert(
        "aqua:acme/other".into(),
        ToolListing {
            source: "test".into(),
            versions: other.to_vec(),
            error: None,
        },
    );
    Listing {
        listed_at: "2026-10-10T00:00:00Z".into(),
        tools,
    }
}

fn run(f: &Fixture, listing: Listing, previous: Option<Index>, now: &str) -> Index {
    run_with(f, listing, previous, now, Attempts::default(), None).0
}

fn run_with(
    f: &Fixture,
    listing: Listing,
    previous: Option<Index>,
    now: &str,
    attempts: Attempts,
    engine_rev: Option<&str>,
) -> (Index, Report, Attempts) {
    let oyzu = fake_oyzu(&f.root);
    build(Inputs {
        config: &f.config,
        registry_path: f.root.join("registry.yaml"),
        listing,
        previous,
        now: now.parse::<Timestamp>().unwrap(),
        oyzu: Some(Oyzu { binary: oyzu }),
        only: None,
        work: f.root.join("work"),
        attempts,
        engine_rev: engine_rev.map(str::to_owned),
    })
    .unwrap()
}

#[test]
fn first_run_verifies_the_newest_releases_and_dates_the_rest() {
    let f = fixture(true);
    let l = listing(
        &[
            listed("v1.3.0-rc.1", Some("2026-10-05T00:00:00Z")),
            listed("v1.2.0", Some("2026-09-01T00:00:00Z")),
            listed("v1.1.0", None),
            listed("v1.0.0", Some("2026-01-01T00:00:00Z")),
            listed("v1.0.0-nightly", None),
        ],
        &[listed("v2", None)],
    );
    let index = run(&f, l, None, "2026-10-10T06:00:00Z");
    index.check().unwrap();
    // The curated name becomes the id.
    let tool = index.tool("oyzu:tool").unwrap();
    assert_eq!(tool.origin, "aqua:acme/tool");
    assert_eq!(tool.serving_class, ServingClass::Cache);
    // The recipe's version_filter drops the nightly; max_versions keeps 3.
    let versions: Vec<_> = tool.versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(versions, ["1.3.0-rc.1", "1.2.0", "1.1.0"]);
    assert_eq!(tool.versions[1].released.source, DateSource::Publisher);
    assert_eq!(tool.versions[2].released.source, DateSource::FirstSeen);
    assert_eq!(tool.versions[2].released.time, "2026-10-10T06:00:00Z");
    // Backfill 2 newest non-prerelease versions; the rc is not verified.
    let checked: Vec<_> = tool
        .versions
        .iter()
        .filter(|v| !v.sets.is_empty())
        .map(|v| v.version.as_str())
        .collect();
    assert_eq!(checked, ["1.2.0", "1.1.0"]);
    assert!(verified(&tool.versions[1]));
    // Not curated: the package id, at the discovery tier and served direct.
    let other = index.tool("oyzu:acme/other").unwrap();
    assert_eq!(other.serving_class, ServingClass::Direct);
    assert!(other.versions.iter().all(|v| v.sets.is_empty()));
    let first = review(None, &index);
    assert_eq!(first.decision, Decision::Review);
    // The pull request body counts every tool of a first index.
    assert_eq!(
        first.tools_with_new_versions.len(),
        index
            .tools
            .iter()
            .filter(|t| !t.versions.is_empty())
            .count()
    );
    assert!(!first.tools_with_new_versions.is_empty());
}

#[test]
fn a_later_run_keeps_dates_and_records_and_only_adds_new_versions() {
    let f = fixture(true);
    let first = run(
        &f,
        listing(&[listed("v1.0.0", None)], &[]),
        None,
        "2026-10-01T00:00:00Z",
    );
    let second = run(
        &f,
        listing(
            &[
                listed("v1.1.0", Some("2026-10-09T00:00:00Z")),
                listed("v1.0.0", None),
            ],
            &[],
        ),
        Some(first.clone()),
        "2026-10-10T00:00:00Z",
    );
    let tool = second.tool("oyzu:tool").unwrap();
    let old = tool.versions.iter().find(|v| v.version == "1.0.0").unwrap();
    assert_eq!(old.released.time, "2026-10-01T00:00:00Z");
    assert_eq!(old.sets, first.tool("oyzu:tool").unwrap().versions[0].sets);
    let r = review(Some(&first), &second);
    assert_eq!(r.decision, Decision::Routine, "{:?}", r.reasons);
    assert_eq!((r.added_versions, r.added_records), (1, 1));
    assert_eq!(review(Some(&second), &second).decision, Decision::NoChange);
}

#[test]
fn new_tools_hosts_classes_and_changed_records_need_review() {
    let f = fixture(true);
    let first = run(
        &f,
        listing(&[listed("v1.0.0", None)], &[]),
        None,
        "2026-10-01T00:00:00Z",
    );
    // A new download host.
    let mut next = first.clone();
    let t = next.tools.iter_mut().find(|t| t.id == "oyzu:tool").unwrap();
    let mut v = t.versions[0].clone();
    v.version = "1.1.0".into();
    v.sets[0]["version"]["normalized"] = "1.1.0".into();
    v.sets[0]["urls"][0]["url"] = "https://evil.example/t.tar.gz".into();
    t.versions.insert(0, v);
    let r = review(Some(&first), &next);
    assert_eq!(r.decision, Decision::Review);
    assert!(
        r.reasons
            .iter()
            .any(|x| x.contains("new host, evil.example")),
        "{:?}",
        r.reasons
    );
    // A changed record for a version already published.
    let mut next = first.clone();
    let t = next.tools.iter_mut().find(|t| t.id == "oyzu:tool").unwrap();
    t.versions[0].sets[0]["platform"] = "linux/arm64/gnu".into();
    assert!(review(Some(&first), &next)
        .reasons
        .iter()
        .any(|x| x.contains("already published")));
    // A serving-class change.
    let unreviewed = run(
        &fixture(false),
        listing(&[listed("v1.0.0", None)], &[]),
        Some(first.clone()),
        "2026-10-01T00:00:00Z",
    );
    assert!(review(Some(&first), &unreviewed)
        .reasons
        .iter()
        .any(|x| x.contains("serving class")));
    // A new tool.
    let mut next = first.clone();
    next.tools.retain(|t| t.id != "oyzu:acme/other");
    assert!(review(Some(&next), &first)
        .reasons
        .iter()
        .any(|x| x.contains("oyzu:acme/other is a new tool")));
}

/// The sample handed to the delivery service and the client: real job
/// output for three tools, trimmed. It must stay a valid index.
#[test]
fn the_sample_index_is_valid() {
    let index = Index::read(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-index.json"),
    )
    .unwrap();
    assert!(index.tool("oyzu:terraform").is_some());
}

const JAVA: &str = "[core.java]\nnames = [\"java\"]\nmax_versions = 5\nverified = true\n";
const TEMURIN: &str = "[tools.\"core:java\"]\nlicense = \"GPL-2.0-only WITH Classpath-exception-2.0\"\nreviewed = false\n";

fn with_java(mut l: Listing, versions: &[&str]) -> Listing {
    l.tools.insert(
        "core:java".into(),
        ToolListing {
            source: "adoptium".into(),
            versions: versions.iter().map(|v| listed(v, None)).collect(),
            error: None,
        },
    );
    l
}

/// Java is indexed as `oyzu:java` with origin `core:java`, and its records
/// come from `oyzu tools evidence --provider java`: the newest releases on the
/// first run, then only new ones.
#[test]
fn java_records_come_from_provider_evidence() {
    let f = fixture_with(true, JAVA, TEMURIN);
    let first = run(
        &f,
        with_java(listing(&[], &[]), &["21.0.8", "21.0.7", "17.0.16"]),
        None,
        "2026-10-01T00:00:00Z",
    );
    first.check().unwrap();
    let java = first.tool("oyzu:java").unwrap();
    assert_eq!(java.origin, "core:java");
    // Unreviewed Temurin stays direct.
    assert_eq!(java.serving_class, ServingClass::Direct);
    let checked: Vec<_> = java
        .versions
        .iter()
        .filter(|v| verified(v))
        .map(|v| v.version.as_str())
        .collect();
    assert_eq!(checked, ["21.0.8", "21.0.7"], "backfill 2");
    assert_eq!(java.versions[0].sets[0]["tool"]["id"], "core:java");
    assert_eq!(java.versions[0].sets[0]["platform"], "linux/amd64/gnu");

    let second = run(
        &f,
        with_java(
            listing(&[], &[]),
            &["21.0.9", "21.0.8", "21.0.7", "17.0.16"],
        ),
        Some(first.clone()),
        "2026-10-10T00:00:00Z",
    );
    let java = second.tool("oyzu:java").unwrap();
    let new = java
        .versions
        .iter()
        .find(|v| v.version == "21.0.9")
        .unwrap();
    assert!(verified(new));
    assert!(java
        .versions
        .iter()
        .find(|v| v.version == "17.0.16")
        .unwrap()
        .sets
        .is_empty());
    let r = review(Some(&first), &second);
    assert_eq!(r.decision, Decision::Routine, "{:?}", r.reasons);

    // Reviewed, Temurin is cached with its source mirrored.
    let reviewed = fixture_with(true, JAVA, &TEMURIN.replace("false", "true"));
    let index = run(
        &reviewed,
        with_java(listing(&[], &[]), &["21.0.8"]),
        None,
        "2026-10-01T00:00:00Z",
    );
    assert_eq!(
        index.tool("oyzu:java").unwrap().serving_class,
        ServingClass::Cache
    );
}

/// Without `verified`, a core provider lists versions only.
#[test]
fn an_unverified_core_provider_lists_versions_only() {
    let f = fixture_with(true, &JAVA.replace("verified = true\n", ""), TEMURIN);
    let index = run(
        &f,
        with_java(listing(&[], &[]), &["21.0.8"]),
        None,
        "2026-10-01T00:00:00Z",
    );
    let java = index.tool("oyzu:java").unwrap();
    assert_eq!(java.versions.len(), 1);
    assert!(java.versions[0].sets.is_empty());
}

/// The budget goes to the launch set in file order, not registry order, and
/// to versions never tried before retries.
#[test]
fn the_budget_follows_the_launch_set_order() {
    let launch = "[[tool]]\npackage = \"acme/other\"\ngroup = \"Test\"\n\
                  [[tool]]\npackage = \"acme/tool\"\nname = \"tool\"\ngroup = \"Test\"\n";
    let f = fixture_full(true, "", "", launch, 2);
    let l = listing(
        &[listed("v1.1.0", None), listed("v1.0.0", None)],
        &[listed("v2.1", None), listed("v2.0", None)],
    );
    let (index, report, _) = run_with(
        &f,
        l,
        None,
        "2026-10-10T00:00:00Z",
        Attempts::default(),
        None,
    );
    let records = |id: &str| {
        index
            .tool(id)
            .unwrap()
            .versions
            .iter()
            .filter(|v| verified(v))
            .count()
    };
    // acme/other comes second in the registry but first in the launch set.
    assert_eq!((records("oyzu:acme/other"), records("oyzu:tool")), (2, 0));
    assert_eq!(report.deferred, 2);
}

/// A version that fails verification is recorded with its reason, waits a
/// week (or for a new oyzu), and its later success merges as routine.
#[test]
fn failed_versions_back_off_and_their_retry_is_routine() {
    let f = fixture(true);
    let l = || listing(&[listed("v1.0.0", None)], &[]);
    std::fs::write(f.root.join("fail"), "").unwrap();
    let (first, _, attempts) = run_with(
        &f,
        l(),
        None,
        "2026-10-01T00:00:00Z",
        Attempts::default(),
        Some("r1"),
    );
    let entry = attempts
        .get("aqua:acme/tool", "1.0.0")
        .expect("the failure is recorded");
    assert_eq!(entry.attempts, 1);
    assert_eq!(entry.reason, "no record reached the verified tier");

    // The next day: nothing spent on it.
    let (_, report, again) = run_with(
        &f,
        l(),
        Some(first.clone()),
        "2026-10-02T00:00:00Z",
        attempts.clone(),
        Some("r1"),
    );
    assert_eq!((report.verified_this_run, report.waiting_retry), (0, 1));
    assert_eq!(again, attempts);

    // A new oyzu retries at once; it still fails, so the count grows.
    let (_, report, bumped) = run_with(
        &f,
        l(),
        Some(first.clone()),
        "2026-10-02T00:00:00Z",
        attempts.clone(),
        Some("r2"),
    );
    assert_eq!(report.verified_this_run, 1);
    assert_eq!(bumped.get("aqua:acme/tool", "1.0.0").unwrap().attempts, 2);

    // A week later it passes: the entry goes, and the change is routine.
    std::fs::remove_file(f.root.join("fail")).unwrap();
    let (fixed, _, after) = run_with(
        &f,
        l(),
        Some(first.clone()),
        "2026-10-08T00:00:00Z",
        attempts,
        Some("r1"),
    );
    assert!(verified(&fixed.tool("oyzu:tool").unwrap().versions[0]));
    assert!(after.tools.is_empty());
    let r = review(Some(&first), &fixed);
    assert_eq!(r.decision, Decision::Routine, "{:?}", r.reasons);
}

/// A reviewed copyleft tool served from Oyzu's host needs its source: the
/// index marks it `source_mirror`, the job asks for each version's archive
/// (GitHub's tag archive by default), and a changed archive needs review.
#[test]
fn copyleft_tools_get_their_source_archives() {
    use oyzu_tool_index::index::Source;
    use oyzu_tool_index::sources::{apply, wanted, Locator};
    let f = fixture_with(true, "", "");
    // MIT: no source needed.
    let mit = run(
        &f,
        listing(&[listed("v1.0.0", None)], &[]),
        None,
        "2026-10-01T00:00:00Z",
    );
    assert!(!mit.tool("oyzu:tool").unwrap().source_mirror);
    assert!(wanted(&mit, &f.config).is_empty());

    let gpl = fixture_full(
        true,
        "",
        "",
        "[[tool]]\npackage = \"acme/tool\"\nname = \"tool\"\ngroup = \"Test\"\n",
        10,
    );
    let mut cfg_text = std::fs::read_to_string(gpl.root.join("config/serving.toml")).unwrap();
    cfg_text = cfg_text.replace("license = \"MIT\"", "license = \"GPL-3.0-only\"");
    std::fs::write(gpl.root.join("config/serving.toml"), cfg_text).unwrap();
    let gpl = Fixture {
        config: Config::load(&gpl.root.join("config")).unwrap(),
        ..gpl
    };
    let mut index = run(
        &gpl,
        listing(&[listed("v1.0.0", None)], &[]),
        None,
        "2026-10-01T00:00:00Z",
    );
    let tool = index.tool("oyzu:tool").unwrap();
    assert!(tool.source_mirror);
    assert_eq!(tool.serving_class, ServingClass::Cache);
    let wants = wanted(&index, &gpl.config);
    assert_eq!(wants.len(), 1);
    assert_eq!(
        (wants[0].tag.as_str(), &wants[0].locator),
        ("v1.0.0", &Locator::GithubArchive)
    );

    let before = index.clone();
    let source = Source {
        url: "https://github.com/acme/tool/archive/refs/tags/v1.0.0.tar.gz".into(),
        sha256: format!("sha256:{}", "a".repeat(64)),
        size: 10,
    };
    assert_eq!(apply(&mut index, &[(wants[0].clone(), source.clone())]), 1);
    index.check().unwrap();
    assert!(wanted(&index, &gpl.config).is_empty());
    // Adding the archive is routine; changing it is not.
    assert_eq!(review(Some(&before), &index).decision, Decision::Routine);
    let mut changed = index.clone();
    changed
        .tools
        .iter_mut()
        .find(|t| t.id == "oyzu:tool")
        .unwrap()
        .versions[0]
        .source
        .as_mut()
        .unwrap()
        .sha256 = format!("sha256:{}", "b".repeat(64));
    assert!(review(Some(&index), &changed)
        .reasons
        .iter()
        .any(|r| r.contains("source archive")));
    // The next run keeps the archive.
    let next = run(
        &gpl,
        listing(&[listed("v1.0.0", None)], &[]),
        Some(index.clone()),
        "2026-10-02T00:00:00Z",
    );
    assert_eq!(
        next.tool("oyzu:tool").unwrap().versions[0].source,
        Some(source)
    );
}

/// A version whose Linux record verified but whose macOS one did not is
/// retried for macOS only: filling it in is routine, changing the published
/// Linux record is not.
#[test]
fn a_retry_fills_in_platforms_without_touching_verified_ones() {
    let f = fixture(true);
    let first = run(
        &f,
        listing(&[listed("v1.0.0", None)], &[]),
        None,
        "2026-10-01T00:00:00Z",
    );
    let mut partial = first.clone();
    let t = partial
        .tools
        .iter_mut()
        .find(|t| t.id == "oyzu:tool")
        .unwrap();
    let mut mac = t.versions[0].sets[0].clone();
    mac["platform"] = "darwin/arm64/native".into();
    mac["tier"] = "discovery".into();
    mac["verified"] = serde_json::json!({"status": "failed"});
    t.versions[0].sets.insert(0, mac.clone());
    assert!(!oyzu_tool_index::index::complete(&t.versions[0]));

    let mut filled = partial.clone();
    let t = filled
        .tools
        .iter_mut()
        .find(|t| t.id == "oyzu:tool")
        .unwrap();
    t.versions[0].sets[0]["tier"] = "verified".into();
    t.versions[0].sets[0]["verified"] = serde_json::json!({"status": "ok"});
    assert!(oyzu_tool_index::index::complete(&t.versions[0]));
    let r = review(Some(&partial), &filled);
    assert_eq!(r.decision, Decision::Routine, "{:?}", r.reasons);
    assert_eq!(r.added_records, 1);

    let mut touched = filled.clone();
    let t = touched
        .tools
        .iter_mut()
        .find(|t| t.id == "oyzu:tool")
        .unwrap();
    t.versions[0].sets[1]["urls"][0]["url"] = "https://github.com/acme/tool/x".into();
    assert!(review(Some(&partial), &touched)
        .reasons
        .iter()
        .any(|x| x.contains("already published")));
}

/// Writes `config/withdrawn.toml` and reloads the configuration.
fn set_withdrawn(f: &mut Fixture, text: &str) {
    let cfg = f.root.join("config");
    std::fs::write(cfg.join("withdrawn.toml"), text).unwrap();
    f.config = Config::load(&cfg).unwrap();
}

fn withdrawal(tool: &str, version: &str, reason: &str) -> String {
    format!("[[version]]\ntool = \"{tool}\"\nversion = \"{version}\"\nreason = \"{reason}\"\n")
}

/// The shipped file has no entries and parses; a missing file withdraws
/// nothing; bad entries are refused.
#[test]
fn withdrawn_toml_is_checked() {
    let shipped = Config::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("config")).unwrap();
    assert!(shipped.withdrawn.versions.is_empty());
    assert!(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("config/withdrawn.toml")
        .exists());

    let mut f = fixture(true);
    assert!(
        f.config.withdrawn.versions.is_empty(),
        "no file, no entries"
    );
    // 512 characters (not bytes) is the limit.
    set_withdrawn(&mut f, &withdrawal("oyzu:tool", "1.0.0", &"é".repeat(512)));
    assert_eq!(f.config.withdrawn.versions.len(), 1);

    let cfg = f.root.join("config");
    let refused = |text: &str| {
        std::fs::write(cfg.join("withdrawn.toml"), text).unwrap();
        format!("{:#}", Config::load(&cfg).err().expect(text))
    };
    assert!(refused(&withdrawal("aqua:acme/tool", "1.0.0", "x")).contains("not an oyzu: tool id"));
    assert!(refused(&withdrawal("oyzu:Tool", "1.0.0", "x")).contains("not an oyzu: tool id"));
    assert!(refused(&withdrawal("oyzu:tool", "1.0.0", "")).contains("reason"));
    assert!(refused(&withdrawal("oyzu:tool", "1.0.0", &"x".repeat(513))).contains("reason"));
    assert!(refused(&withdrawal("oyzu:tool", "", "x")).contains("empty version"));
    let twice = withdrawal("oyzu:tool", "1.0.0", "a") + &withdrawal("oyzu:tool", "1.0.0", "b");
    assert!(refused(&twice).contains("withdrawn twice"));
    assert!(refused(
        "[[version]]\ntool = \"oyzu:tool\"\nversion = \"1\"\nreason = \"x\"\nnote = \"y\"\n"
    )
    .contains("parsing"));
}

/// `withdrawn` is omitted when unset, round-trips when set, and its reason
/// must be 1 to 512 characters.
#[test]
fn the_index_checks_withdrawal_reasons() {
    use oyzu_tool_index::index::Withdrawn;
    let f = fixture(true);
    let mut index = run(
        &f,
        listing(&[listed("v1.0.0", None)], &[]),
        None,
        "2026-10-01T00:00:00Z",
    );
    let bytes = index.to_bytes().unwrap();
    assert!(!String::from_utf8(bytes).unwrap().contains("withdrawn"));
    let set = |index: &mut Index, reason: String| {
        index
            .tools
            .iter_mut()
            .find(|t| t.id == "oyzu:tool")
            .unwrap()
            .versions[0]
            .withdrawn = Some(Withdrawn { reason });
    };
    set(&mut index, "upstream assets deleted (404)".into());
    index.check().unwrap();
    let text = String::from_utf8(index.to_bytes().unwrap()).unwrap();
    assert!(text
        .contains("\"withdrawn\": {\n            \"reason\": \"upstream assets deleted (404)\""));
    let back: Index = serde_json::from_str(&text).unwrap();
    assert_eq!(back, index);
    set(&mut index, "ü".repeat(512));
    index.check().unwrap();
    set(&mut index, String::new());
    assert!(index.check().is_err());
    set(&mut index, "x".repeat(513));
    assert!(index.check().is_err());
    // Unknown fields inside `withdrawn` are refused, like everywhere else.
    let bad = text.replace(
        "\"reason\": \"upstream assets deleted (404)\"",
        "\"reason\": \"x\", \"by\": \"y\"",
    );
    assert!(serde_json::from_str::<Index>(&bad).is_err());
}

/// A withdrawn version stays in the index with its records even after
/// upstream stops listing it and `max_versions` would trim it, and its
/// records do not age out under `keep_records`. Withdrawing needs review.
#[test]
fn a_withdrawn_version_is_kept_with_its_records() {
    let mut f = fixture(true);
    let first = run(
        &f,
        listing(
            &[
                listed("v1.2.0", None),
                listed("v1.1.0", None),
                listed("v1.0.0", None),
            ],
            &[],
        ),
        None,
        "2026-10-01T00:00:00Z",
    );
    let old = first.tool("oyzu:tool").unwrap().versions[1].clone();
    assert_eq!(old.version, "1.1.0");
    assert!(verified(&old));

    set_withdrawn(&mut f, &withdrawal("oyzu:tool", "1.1.0", "assets deleted"));
    // Upstream drops 1.1.0 and 1.0.0; three newer releases fill max_versions.
    let (second, report, _) = run_with(
        &f,
        listing(
            &[
                listed("v1.5.0", None),
                listed("v1.4.0", None),
                listed("v1.3.0", None),
                listed("v1.2.0", None),
            ],
            &[],
        ),
        Some(first.clone()),
        "2026-10-02T00:00:00Z",
        Attempts::default(),
        None,
    );
    second.check().unwrap();
    let tool = second.tool("oyzu:tool").unwrap();
    let names: Vec<_> = tool.versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(names, ["1.5.0", "1.4.0", "1.3.0", "1.1.0"]);
    let w = &tool.versions[3];
    assert_eq!(w.withdrawn.as_ref().unwrap().reason, "assets deleted");
    assert_eq!(w.sets, old.sets, "records kept, not re-verified");
    assert_eq!(w.released, old.released);
    // keep_records = 2 counts only the versions that are not withdrawn.
    let with_records: Vec<_> = tool
        .versions
        .iter()
        .filter(|v| !v.sets.is_empty())
        .map(|v| v.version.as_str())
        .collect();
    assert_eq!(with_records, ["1.5.0", "1.4.0", "1.1.0"]);
    assert_eq!(report.verified_this_run, 3);
    assert_eq!(report.withdrawn, 1);
    assert!(report.withdrawals_not_indexed.is_empty());

    let r = review(Some(&first), &second);
    assert_eq!(r.decision, Decision::Review);
    assert!(
        r.reasons
            .contains(&"oyzu:tool 1.1.0 is withdrawn: assets deleted".to_owned()),
        "{:?}",
        r.reasons
    );
    assert!(oyzu_tool_index::review::markdown(&r)
        .contains("- oyzu:tool 1.1.0 is withdrawn: assets deleted\n"));

    // The next run keeps it as it is: no change.
    let third = run(
        &f,
        listing(
            &[
                listed("v1.5.0", None),
                listed("v1.4.0", None),
                listed("v1.3.0", None),
            ],
            &[],
        ),
        Some(second.clone()),
        "2026-10-03T00:00:00Z",
    );
    assert_eq!(third, second);
    assert_eq!(review(Some(&second), &third).decision, Decision::NoChange);

    // Lifting the withdrawal lets the version age out again, which needs
    // review because it was verified.
    set_withdrawn(&mut f, "");
    let fourth = run(
        &f,
        listing(&[listed("v1.5.0", None)], &[]),
        Some(third.clone()),
        "2026-10-04T00:00:00Z",
    );
    assert!(fourth
        .tool("oyzu:tool")
        .unwrap()
        .versions
        .iter()
        .all(|v| v.version != "1.1.0" && v.withdrawn.is_none()));
    assert_eq!(review(Some(&third), &fourth).decision, Decision::Review);
}

/// A withdrawn version is never verified: not as a failed version due for a
/// retry, not as a new one, and its failure record is forgotten.
#[test]
fn a_withdrawn_version_is_never_verified() {
    let mut f = fixture(true);
    let l = || listing(&[listed("v1.0.0", None)], &[]);
    std::fs::write(f.root.join("fail"), "").unwrap();
    let (first, _, attempts) = run_with(
        &f,
        l(),
        None,
        "2026-10-01T00:00:00Z",
        Attempts::default(),
        Some("r1"),
    );
    assert!(attempts.get("aqua:acme/tool", "1.0.0").is_some());
    std::fs::remove_file(f.root.join("fail")).unwrap();

    set_withdrawn(&mut f, &withdrawal("oyzu:tool", "1.0.0", "broken release"));
    // A new oyzu would retry at once, and a week later it would be due.
    for (now, engine) in [
        ("2026-10-02T00:00:00Z", "r2"),
        ("2026-10-09T00:00:00Z", "r1"),
    ] {
        let (index, report, after) = run_with(
            &f,
            l(),
            Some(first.clone()),
            now,
            attempts.clone(),
            Some(engine),
        );
        assert_eq!(
            (report.verified_this_run, report.waiting_retry),
            (0, 0),
            "{engine}"
        );
        assert!(after.tools.is_empty(), "the failure is forgotten");
        let v = &index.tool("oyzu:tool").unwrap().versions[0];
        assert!(!verified(v));
        assert_eq!(v.withdrawn.as_ref().unwrap().reason, "broken release");
    }

    // On a tool's first run, a withdrawn version is skipped and the backfill
    // takes the next releases.
    let mut g = fixture(true);
    set_withdrawn(&mut g, &withdrawal("oyzu:tool", "1.2.0", "bad"));
    let index = run(
        &g,
        listing(
            &[
                listed("v1.2.0", None),
                listed("v1.1.0", None),
                listed("v1.0.0", None),
            ],
            &[],
        ),
        None,
        "2026-10-01T00:00:00Z",
    );
    let checked: Vec<_> = index
        .tool("oyzu:tool")
        .unwrap()
        .versions
        .iter()
        .map(|v| (v.version.as_str(), verified(v), v.withdrawn.is_some()))
        .collect();
    assert_eq!(
        checked,
        [
            ("1.2.0", false, true),
            ("1.1.0", true, false),
            ("1.0.0", true, false)
        ]
    );
}

/// A withdrawal of a tool or version the index does not list does not fail
/// the build; the report and the pull request body name it.
#[test]
fn a_withdrawal_the_index_does_not_list_is_noted() {
    use oyzu_tool_index::review::{markdown, review_with};
    let mut f = fixture(true);
    set_withdrawn(
        &mut f,
        &(withdrawal("oyzu:nope", "1.0.0", "x") + &withdrawal("oyzu:tool", "9.9.9", "y")),
    );
    let (index, report, _) = run_with(
        &f,
        listing(&[listed("v1.0.0", None)], &[]),
        None,
        "2026-10-01T00:00:00Z",
        Attempts::default(),
        None,
    );
    assert_eq!(
        report.withdrawals_not_indexed,
        ["oyzu:nope 1.0.0", "oyzu:tool 9.9.9"]
    );
    assert_eq!(report.withdrawn, 0);
    let r = review_with(Some(&index), &index, &f.config.withdrawn);
    assert_eq!(r.decision, Decision::NoChange);
    assert_eq!(r.notes.len(), 2);
    let body = markdown(&r);
    assert!(
        body.contains("config/withdrawn.toml names oyzu:tool 9.9.9, which the index does not list"),
        "{body}"
    );
}

/// Adding, lifting or rewording a withdrawal each needs a person, with a
/// line in the pull request body.
#[test]
fn every_withdrawal_change_needs_review() {
    let mut f = fixture(true);
    let l = || listing(&[listed("v1.1.0", None), listed("v1.0.0", None)], &[]);
    let plain = run(&f, l(), None, "2026-10-01T00:00:00Z");
    set_withdrawn(&mut f, &withdrawal("oyzu:tool", "1.0.0", "first reason"));
    let withdrawn = run(&f, l(), Some(plain.clone()), "2026-10-02T00:00:00Z");
    set_withdrawn(&mut f, &withdrawal("oyzu:tool", "1.0.0", "second reason"));
    let reworded = run(&f, l(), Some(withdrawn.clone()), "2026-10-03T00:00:00Z");
    set_withdrawn(&mut f, "");
    let lifted = run(&f, l(), Some(reworded.clone()), "2026-10-04T00:00:00Z");
    assert_eq!(lifted, plain, "lifting restores the index byte for byte");

    for (before, after, line) in [
        (
            &plain,
            &withdrawn,
            "oyzu:tool 1.0.0 is withdrawn: first reason",
        ),
        (
            &withdrawn,
            &reworded,
            "oyzu:tool 1.0.0 changes its withdrawal reason to: second reason",
        ),
        (&reworded, &lifted, "oyzu:tool 1.0.0 is no longer withdrawn"),
    ] {
        let r = review(Some(before), after);
        assert_eq!(r.decision, Decision::Review, "{line}");
        assert_eq!(r.reasons, [line.to_owned()]);
        assert!(oyzu_tool_index::review::markdown(&r).contains(&format!("- {line}\n")));
    }

    // A withdrawn version that leaves the index needs review even without
    // records.
    let mut gone = withdrawn.clone();
    let t = gone.tools.iter_mut().find(|t| t.id == "oyzu:tool").unwrap();
    t.versions.retain(|v| v.version != "1.0.0");
    let mut unverified = withdrawn.clone();
    let t = unverified
        .tools
        .iter_mut()
        .find(|t| t.id == "oyzu:tool")
        .unwrap();
    t.versions[1].sets.clear();
    assert!(review(Some(&unverified), &gone)
        .reasons
        .contains(&"oyzu:tool 1.0.0 is removed while withdrawn".to_owned()));
}
