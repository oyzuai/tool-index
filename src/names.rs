//! Short names (`gh`, `terraform`) from the pinned mise name catalog, read
//! as data exactly as oyzu's `short_names.rs` does: a name expands to its
//! first `aqua:` backend (without `[options]`), so the index and the CLI agree.
use anyhow::{ensure, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;

const MAX_ENTRY_BYTES: u64 = 1 << 20;

#[derive(Deserialize, Default)]
struct Entry {
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    backends: Vec<toml::Value>,
}

fn backend_text(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(text) => Some(text.clone()),
        toml::Value::Table(table) => table.get("full")?.as_str().map(str::to_owned),
        _ => None,
    }
}

/// Short name to aqua package, for every name whose first supported backend
/// is an aqua package.
pub fn parse_catalog(archive: &[u8], version: &str) -> Result<BTreeMap<String, String>> {
    let registry_dir = format!("mise-{version}/registry/");
    let mut names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut tar = tar::Archive::new(GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        let Some(stem) = path
            .strip_prefix(&registry_dir)
            .and_then(|rest| rest.strip_suffix(".toml"))
            .filter(|stem| !stem.is_empty() && !stem.contains('/'))
        else {
            continue;
        };
        ensure!(
            entry.size() <= MAX_ENTRY_BYTES,
            "catalog entry {stem} is unexpectedly large"
        );
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        let Ok(parsed) = toml::from_str::<Entry>(&text) else {
            continue;
        };
        let backends: Vec<String> = parsed.backends.iter().filter_map(backend_text).collect();
        for name in std::iter::once(stem.to_owned()).chain(parsed.aliases) {
            names.entry(name).or_insert_with(|| backends.clone());
        }
    }
    ensure!(!names.is_empty(), "the name catalog holds no tools");
    Ok(names
        .into_iter()
        .filter_map(|(name, backends)| {
            backends
                .iter()
                .find_map(|b| {
                    b.strip_prefix("aqua:")
                        .filter(|p| !p.contains('['))
                        .map(str::to_owned)
                })
                .map(|package| (name, package))
        })
        .collect())
}

/// Package to its short names, sorted.
pub fn by_package(names: &BTreeMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, package) in names {
        out.entry(package.clone()).or_default().push(name.clone());
    }
    out
}

#[cfg(test)]
pub(crate) fn test_archive(version: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for (name, text) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("mise-{version}/registry/{name}.toml"),
                    text.as_bytes(),
                )
                .unwrap();
        }
        builder.finish().unwrap();
    }
    let mut out = Vec::new();
    let mut encoder = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
    std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
    encoder.finish().unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_aqua_backend_wins() {
        let archive = test_archive(
            "1.0",
            &[
                (
                    "github-cli",
                    "aliases = [\"gh\"]\nbackends = [\"asdf:x/y\", \"aqua:cli/cli\"]\n",
                ),
                ("viu", "backends = [\"cargo:viu\"]\n"),
                ("table", "backends = [{ full = \"aqua:a/b\" }]\n"),
                ("opts", "backends = [\"aqua:c/d[x=1]\", \"aqua:c/e\"]\n"),
            ],
        );
        let names = parse_catalog(&archive, "1.0").unwrap();
        assert_eq!(names["gh"], "cli/cli");
        assert_eq!(names["github-cli"], "cli/cli");
        assert_eq!(names["table"], "a/b");
        assert_eq!(names["opts"], "c/e");
        assert!(!names.contains_key("viu"));
        assert_eq!(by_package(&names)["cli/cli"], vec!["gh", "github-cli"]);
    }
}
