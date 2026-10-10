//! The build and review steps on a two-package snapshot, with a stand-in
//! `oyzu` that emits fixed records, so no network is needed.
use jiff::Timestamp;
use oyzu_tool_index::build::{build, Inputs};
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
if [ "$3" = "evidence" ]; then
  sed 's/"tier":"discovery"/"tier":"verified"/g' "$5"
  exit 0
fi
sets=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--version" ]; then
    n="${a#v}"
    r="{\"tool\":{\"id\":\"aqua:acme/tool\"},\"version\":{\"requested\":\"$a\",\"normalized\":\"$n\"},\"platform\":\"linux/amd64/gnu\",\"status\":\"resolved\",\"tier\":\"discovery\",\"urls\":[{\"role\":\"artifact\",\"url\":\"https://github.com/acme/tool/releases/download/$a/t.tar.gz\"}]}"
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
max_versions_per_run = 10
keep_records = 2
include_prereleases = false
[signing]
expires_days = 14
"#,
            digest(REGISTRY.as_bytes()),
            digest(&names)
        ),
    )
    .unwrap();
    std::fs::write(
        cfg.join("launch-set.toml"),
        "[[tool]]\npackage = \"acme/tool\"\nname = \"tool\"\ngroup = \"Test\"\n",
    )
    .unwrap();
    std::fs::write(
        cfg.join("serving.toml"),
        format!("[tools.\"aqua:acme/tool\"]\nlicense = \"MIT\"\nreviewed = {reviewed}\n"),
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
    })
    .unwrap()
    .0
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
    assert_eq!(review(None, &index).decision, Decision::Review);
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
