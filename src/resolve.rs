//! Running the public `oyzu tools resolve` and `oyzu tools evidence` commands,
//! so the index holds exactly the records a user would get running them on
//! the same snapshot (TOOLIDX-01).
use crate::config::RegistryPin;
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Oyzu {
    pub binary: PathBuf,
}

/// Exit 0 is a complete document; exit 3 is a complete document with error
/// records or records that stayed at the discovery tier. Both carry output.
fn run(oyzu: &Oyzu, args: &[String], out: &Path) -> Result<Value> {
    let output = Command::new(&oyzu.binary)
        .arg("--json")
        .args(args)
        .output()
        .with_context(|| format!("running {}", oyzu.binary.display()))?;
    let code = output.status.code().unwrap_or(-1);
    std::fs::write(out, &output.stdout)?;
    if code != 0 && code != 3 {
        bail!(
            "`oyzu {}` exited {code}: {}",
            args.first().map(String::as_str).unwrap_or(""),
            String::from_utf8_lossy(&output.stdout)
                .chars()
                .take(400)
                .collect::<String>()
        );
    }
    let doc: Value = serde_json::from_slice(&output.stdout).context("parsing oyzu output")?;
    ensure!(
        doc["kind"] == "artifact-sets",
        "oyzu did not return an artifact-set document"
    );
    Ok(doc)
}

pub fn resolve_and_verify(
    oyzu: &Oyzu,
    registry: &Path,
    pin: &RegistryPin,
    package: &str,
    versions: &[String],
    platforms: &[String],
    work: &Path,
) -> Result<Value> {
    let stem = package.replace('/', "__");
    let discovery = work.join(format!("{stem}.discovery.json"));
    let mut args: Vec<String> = vec![
        "tools".into(),
        "resolve".into(),
        "--registry".into(),
        registry.display().to_string(),
        "--registry-digest".into(),
        pin.digest.clone(),
        "--snapshot-tag".into(),
        pin.tag.clone(),
        "--tool".into(),
        package.into(),
    ];
    if let Some(commit) = &pin.commit {
        args.extend(["--snapshot-commit".into(), commit.clone()]);
    }
    for v in versions {
        args.extend(["--version".into(), v.clone()]);
    }
    for p in platforms {
        args.extend(["--platform".into(), p.clone()]);
    }
    run(oyzu, &args, &discovery)?;
    let verified = work.join(format!("{stem}.verified.json"));
    let args = vec![
        "tools".into(),
        "evidence".into(),
        "--set".into(),
        discovery.display().to_string(),
    ];
    run(oyzu, &args, &verified)
}
