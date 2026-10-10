//! The few package facts the job needs from the pinned aqua registry snapshot
//! to list versions. Recipe evaluation itself is `oyzu-recipe`'s job.
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize)]
struct File {
    packages: Vec<serde_yaml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub source_kind: String,
    /// `owner/repo` on GitHub, where versions are listed.
    pub repository: Option<String>,
    /// `github_tag` lists tags; otherwise releases are listed.
    pub version_source: Option<String>,
}

pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn check_digest(bytes: &[u8], pinned: &str) -> Result<()> {
    let actual = digest(bytes);
    ensure!(
        actual == pinned,
        "registry snapshot digest {actual} does not match the pinned {pinned}"
    );
    Ok(())
}

fn text(v: &serde_yaml::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_owned)
}

pub fn packages(bytes: &[u8]) -> Result<Vec<Package>> {
    let file: File = serde_yaml::from_slice(bytes).context("parsing the registry snapshot")?;
    let mut out = Vec::with_capacity(file.packages.len());
    for p in &file.packages {
        let owner = text(p, "repo_owner");
        let repo = text(p, "repo_name");
        let repository = owner.zip(repo).map(|(o, r)| format!("{o}/{r}"));
        let Some(name) = text(p, "name").or_else(|| repository.clone()) else {
            continue;
        };
        out.push(Package {
            name,
            source_kind: text(p, "type").unwrap_or_else(|| "unknown".into()),
            repository,
            version_source: text(p, "version_source"),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_repositories_and_version_sources() {
        let yaml = b"packages:\n  - type: github_release\n    repo_owner: cli\n    repo_name: cli\n  - name: kubernetes/kubernetes/kubectl\n    type: http\n    repo_owner: kubernetes\n    repo_name: kubernetes\n  - type: go_install\n    repo_owner: a\n    repo_name: b\n    version_source: github_tag\n";
        let p = packages(yaml).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].name, "a/b");
        assert_eq!(p[0].version_source.as_deref(), Some("github_tag"));
        assert_eq!(p[2].name, "kubernetes/kubernetes/kubectl");
        assert_eq!(p[2].repository.as_deref(), Some("kubernetes/kubernetes"));
    }
}
