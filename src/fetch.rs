//! Downloading the pinned inputs: the aqua registry snapshot and its license,
//! and the mise name catalog. Each pinned file is checked against its digest.
use crate::config::{expand, IndexConfig};
use crate::registry;
use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};

pub struct Fetched {
    pub registry: PathBuf,
    pub notice: PathBuf,
    pub names: PathBuf,
}

const MAX_BYTES: usize = 64 << 20;

async fn get(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    ensure!(
        response.status().is_success(),
        "{url} returned HTTP {}",
        response.status()
    );
    let bytes = response.bytes().await?;
    ensure!(bytes.len() <= MAX_BYTES, "{url} is unexpectedly large");
    Ok(bytes.to_vec())
}

pub fn notice_text(source: &str, tag: &str, license: &str) -> String {
    format!(
        "Oyzu tool index\n\n\
         This index contains data derived from the aqua registry\n\
         (https://github.com/{source}) at tag {tag}, which is distributed\n\
         under the following license.\n\n\
         ----------------------------------------------------------------\n\n\
         {}\n",
        license.trim_end()
    )
}

pub async fn fetch(cfg: &IndexConfig, work: &Path) -> Result<Fetched> {
    std::fs::create_dir_all(work)?;
    let client = reqwest::Client::builder()
        .user_agent("oyzu-tool-index")
        .build()?;
    let tag = &cfg.aqua_registry.tag;

    let registry_bytes = get(&client, &expand(&cfg.aqua_registry.registry_url, tag)).await?;
    registry::check_digest(&registry_bytes, &cfg.aqua_registry.digest)?;
    let registry_path = work.join("registry.yaml");
    std::fs::write(&registry_path, &registry_bytes)?;

    let license = get(&client, &expand(&cfg.aqua_registry.license_url, tag)).await?;
    let notice = work.join("NOTICE");
    std::fs::write(
        &notice,
        notice_text(
            &cfg.aqua_registry.source,
            tag,
            &String::from_utf8_lossy(&license),
        ),
    )?;

    let names_bytes = get(&client, &cfg.names.url).await?;
    registry::check_digest(&names_bytes, &cfg.names.digest).context("name catalog")?;
    let names = work.join("names.crate");
    std::fs::write(&names, &names_bytes)?;

    Ok(Fetched {
        registry: registry_path,
        notice,
        names,
    })
}
