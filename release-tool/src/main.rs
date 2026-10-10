//! `tool-index-release`: the release workflow's steps (`serial`, `stamp`,
//! `release`), built without private oyzuai/oyzu so the daily re-sign does not
//! depend on the oyzu read App. The modules are the parent crate's, by path.
#[allow(dead_code)]
#[path = "../../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../../src/index.rs"]
mod index;
#[allow(dead_code)]
#[path = "../../src/release.rs"]
mod release;
#[allow(dead_code)]
#[path = "../../src/serial.rs"]
mod serial;
#[allow(dead_code)]
#[path = "../../src/stamp.rs"]
mod stamp;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use jiff::Timestamp;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "tool-index-release",
    about = "Stamp and check Oyzu's public tool index for signed release"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the highest `index-v<serial>` among the tags in a file (one per
    /// line), or 0 before the first release. Fails on a gap or an odd tag.
    Serial {
        #[arg(long)]
        tags: PathBuf,
    },
    /// Stamp a release copy of the committed index with `serial` and `expires`.
    Stamp {
        #[arg(long, default_value = "config")]
        config: PathBuf,
        /// The committed, unstamped index.
        #[arg(long, default_value = "index/index.json")]
        index: PathBuf,
        /// The `index.json` of the release with the highest serial, if any.
        #[arg(long)]
        published: Option<PathBuf>,
        /// The repository's tags, one per line; `published` must carry the
        /// highest `index-v<serial>` among them.
        #[arg(long)]
        tags: Option<PathBuf>,
        /// Fixed clock for reproducible runs (RFC 3339).
        #[arg(long)]
        now: Option<Timestamp>,
        /// Where the stamped copy goes.
        #[arg(long)]
        out: PathBuf,
    },
    /// Check a release directory (index.json, index.json.zst,
    /// index.json.sigstore.json, NOTICE) and print the plan for `index-v<serial>`.
    Release {
        #[arg(long)]
        dir: PathBuf,
        /// Report missing assets instead of failing.
        #[arg(long)]
        dry_run: bool,
        /// Fixed clock for the expiry check (RFC 3339).
        #[arg(long)]
        now: Option<Timestamp>,
    },
}

fn read(path: &PathBuf) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

fn tags(path: &PathBuf) -> Result<Vec<String>> {
    Ok(String::from_utf8(read(path)?)?
        .lines()
        .map(str::to_owned)
        .collect())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Serial { tags: path } => {
            println!("{}", serial::latest(&tags(&path)?)?.unwrap_or(0));
        }
        Command::Stamp {
            config,
            index,
            published,
            tags: tag_file,
            now,
            out,
        } => {
            let cfg = config::Config::load(&config)?;
            let latest = tag_file
                .map(|p| tags(&p).and_then(|t| serial::latest(&t)))
                .transpose()?
                .flatten();
            let published = published.as_ref().map(read).transpose()?;
            let bytes = stamp::stamp_after(
                &read(&index)?,
                published.as_deref(),
                latest,
                now.unwrap_or_else(Timestamp::now),
                cfg.index.signing.expires_days,
            )?;
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
            let stamped: index::Index = serde_json::from_slice(&bytes)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "serial": stamped.serial,
                    "expires": stamped.expires,
                }))?
            );
        }
        Command::Release { dir, dry_run, now } => {
            let plan = release::plan(&dir, now.unwrap_or_else(Timestamp::now), dry_run)?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
    }
    Ok(())
}
