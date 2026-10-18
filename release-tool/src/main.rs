//! `tool-index-release`: the release workflow's steps (`serial`, `stamp`,
//! `plugins`, `release`), built without private oyzuai/oyzu so the daily re-sign does not
//! depend on the oyzu read App. The modules are the parent crate's, by path.
#[allow(dead_code)]
#[path = "../../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../../src/index.rs"]
mod index;
#[allow(dead_code)]
#[path = "../../src/plugins.rs"]
mod plugins;
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
    /// The signed plugin catalog (`plugins.json`), published beside
    /// `index.json`. A failure here never concerns the tools release.
    Plugins {
        #[command(subcommand)]
        command: PluginsCommand,
    },
    /// Check a release directory (index.json, index.json.zst,
    /// index.json.sigstore.json, NOTICE, and plugins.json with its bundle when
    /// present) and print the plan for `index-v<serial>`.
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

#[derive(Subcommand)]
enum PluginsCommand {
    /// List the source's releases that must be verified available before they
    /// are advertised, as JSON (the input of the availability script).
    Candidates {
        #[arg(long, default_value = "config/plugins.toml")]
        source: PathBuf,
    },
    /// Write `plugins.json` for the stamped index: serial from `index.json`,
    /// `generated_at` now, `expires_at` the index's expiry cut to 31 days.
    /// Writes nothing on failure.
    Generate {
        #[arg(long, default_value = "config/plugins.toml")]
        source: PathBuf,
        /// The stamped release `index.json`.
        #[arg(long)]
        index: PathBuf,
        /// Releases a verifier found available; without it none are advertised.
        #[arg(long)]
        verified: Option<PathBuf>,
        /// Fixed clock for reproducible runs (RFC 3339).
        #[arg(long)]
        now: Option<Timestamp>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Check a `plugins.json` against the contract and the stamped index.
    Check {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        index: PathBuf,
        #[arg(long)]
        now: Option<Timestamp>,
    },
    /// Print the cosign statement that names `plugins.json`'s exact bytes.
    Statement {
        #[arg(long)]
        file: PathBuf,
    },
    /// Summarize the differences between two versions of the source for
    /// review; capability-sensitive changes come first.
    Review {
        /// The base branch's source; omit for a first source.
        #[arg(long)]
        previous: Option<PathBuf>,
        #[arg(long, default_value = "config/plugins.toml")]
        next: PathBuf,
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
        Command::Plugins { command } => plugins_command(command)?,
        Command::Release { dir, dry_run, now } => {
            let plan = release::plan(&dir, now.unwrap_or_else(Timestamp::now), dry_run)?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
    }
    Ok(())
}

fn text(path: &PathBuf) -> Result<String> {
    String::from_utf8(read(path)?).with_context(|| format!("{} is not UTF-8", path.display()))
}

fn stamped_index(path: &PathBuf) -> Result<(u64, Timestamp)> {
    let index: index::Index = serde_json::from_slice(&read(path)?).context("parsing index.json")?;
    index.check()?;
    let (Some(serial), Some(expires)) = (index.serial, index.expires) else {
        anyhow::bail!("{} is not stamped", path.display());
    };
    Ok((serial, expires.parse().context("index.json expires")?))
}

fn plugins_command(command: PluginsCommand) -> Result<()> {
    match command {
        PluginsCommand::Candidates { source } => {
            let content = plugins::Source::parse(&text(&source)?)?.content();
            content.check().context("checking the plugin source")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&plugins::candidates(&content))?
            );
        }
        PluginsCommand::Generate {
            source,
            index,
            verified,
            now,
            out,
        } => {
            let (serial, expires) = stamped_index(&index)?;
            let verified = verified.as_ref().map(read).transpose()?;
            let generated = plugins::generate(
                &text(&source)?,
                verified.as_deref(),
                serial,
                now.unwrap_or_else(Timestamp::now),
                expires,
            )?;
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, &generated.bytes)
                .with_context(|| format!("writing {}", out.display()))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "serial": generated.catalog.serial,
                    "generated_at": generated.catalog.generated_at,
                    "expires_at": generated.catalog.expires_at,
                    "content_digest": generated.catalog.content_digest,
                    "packages": generated.catalog.content.packages.len(),
                    "omitted": generated.omitted,
                }))?
            );
        }
        PluginsCommand::Check { file, index, now } => {
            let (serial, _) = stamped_index(&index)?;
            let catalog =
                plugins::check_release(&read(&file)?, serial, now.unwrap_or_else(Timestamp::now))?;
            println!("{} ({})", catalog.content_digest, catalog.expires_at);
        }
        PluginsCommand::Statement { file } => {
            use sha2::{Digest, Sha256};
            println!(
                "{}",
                serde_json::json!({
                    "_type": "https://in-toto.io/Statement/v1",
                    "subject": [{
                        "name": release::PLUGINS,
                        "digest": {"sha256": hex::encode(Sha256::digest(read(&file)?))},
                    }],
                    "predicateType": "https://sigstore.dev/cosign/sign/v1",
                    "predicate": {},
                })
            );
        }
        PluginsCommand::Review { previous, next } => {
            let previous = previous.as_ref().map(text).transpose()?;
            let changes = plugins::review(previous.as_deref(), &text(&next)?)?;
            println!("{}", serde_json::to_string_pretty(&changes)?);
        }
    }
    Ok(())
}
