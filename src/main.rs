//! `tool-index`: builds, reviews and stamps Oyzu's public tool index.
//!
//! A daily run is `fetch`, `list`, `build` and `review` (the job opens a pull
//! request with the result). Releasing (`serial`, `stamp`, `release`) is the
//! separate `tool-index-release` binary in `release-tool/`, which builds without
//! private oyzu. See README.md.
use oyzu_tool_index::{
    attempts, build, config, fetch, index, registry, resolve, review, sources, versions,
};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use jiff::Timestamp;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "tool-index",
    about = "Build, review and stamp Oyzu's public tool index for release"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download the pinned registry snapshot, its license and the name catalog.
    Fetch {
        #[arg(long, default_value = "config")]
        config: PathBuf,
        #[arg(long, default_value = "work")]
        work: PathBuf,
    },
    /// List upstream versions into a listing file.
    List {
        #[arg(long, default_value = "config")]
        config: PathBuf,
        #[arg(long, default_value = "work")]
        work: PathBuf,
        /// `github` (GraphQL, needs GITHUB_TOKEN) or `git` (tags, no dates).
        #[arg(long, default_value = "github")]
        source: String,
        #[arg(long, default_value = "https://api.github.com/graphql")]
        github_api: String,
        #[arg(long, default_value = "https://api.adoptium.net")]
        adoptium: String,
        /// Restrict to these tool ids (repeatable).
        #[arg(long)]
        only: Vec<String>,
        /// Only the launch set and core providers.
        #[arg(long)]
        launch_set_only: bool,
        #[arg(long, default_value = "listing.json")]
        out: PathBuf,
    },
    /// Build index.json from the listing, the previous index and, for the
    /// verified tier, `oyzu tools resolve` and `oyzu tools evidence`.
    Build {
        #[arg(long, default_value = "config")]
        config: PathBuf,
        #[arg(long, default_value = "work")]
        work: PathBuf,
        #[arg(long)]
        listing: PathBuf,
        /// The index currently published (the job's only state).
        #[arg(long)]
        previous: Option<PathBuf>,
        /// The oyzu binary that runs resolve and evidence.
        #[arg(long, default_value = "oyzu")]
        oyzu: PathBuf,
        /// Skip resolve and evidence; carry verified records forward only.
        #[arg(long)]
        no_verify: bool,
        /// Versions that failed verification before; the updated file is
        /// written to `<out>/attempts.json`.
        #[arg(long, default_value = "index/attempts.json")]
        attempts: PathBuf,
        /// The oyzu revision `--oyzu` was built from. Failures recorded under
        /// another revision are retried at once.
        #[arg(long)]
        engine_rev: Option<String>,
        /// Adoptium API, for Temurin's source archives.
        #[arg(long, default_value = "https://api.adoptium.net")]
        adoptium: String,
        #[arg(long)]
        only: Vec<String>,
        /// Fixed clock for reproducible runs (RFC 3339).
        #[arg(long)]
        now: Option<Timestamp>,
        /// Output directory for index.json and NOTICE.
        #[arg(long, default_value = "index")]
        out: PathBuf,
    },
    /// Decide whether an index change is routine or needs a person.
    Review {
        #[arg(long)]
        previous: Option<PathBuf>,
        #[arg(long)]
        next: PathBuf,
        /// Also write the pull request body here.
        #[arg(long)]
        markdown: Option<PathBuf>,
    },
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Fetch { config, work } => {
            let cfg = config::Config::load(&config)?;
            let f = fetch::fetch(&cfg.index, &work).await?;
            println!(
                "{}\n{}\n{}",
                f.registry.display(),
                f.notice.display(),
                f.names.display()
            );
        }
        Command::List {
            config,
            work,
            source,
            github_api,
            adoptium,
            only,
            launch_set_only,
            out,
        } => {
            let cfg = config::Config::load(&config)?;
            let bytes = std::fs::read(work.join("registry.yaml")).context("run `fetch` first")?;
            registry::check_digest(&bytes, &cfg.index.aqua_registry.digest)?;
            let launch = cfg.launch_packages();
            let only: BTreeSet<String> = only.into_iter().collect();
            let packages: Vec<registry::Package> = registry::packages(&bytes)?
                .into_iter()
                .filter(|p| only.is_empty() || only.contains(&versions::aqua_id(&p.name)))
                .filter(|p| !launch_set_only || launch.contains(&p.name))
                .filter(|p| {
                    launch.contains(&p.name)
                        || cfg.index.discovery.source_kinds.contains(&p.source_kind)
                })
                .collect();
            let mut tools = match source.as_str() {
                "github" => {
                    let token = std::env::var("GITHUB_TOKEN")
                        .context("GITHUB_TOKEN is required for --source github")?;
                    let mut tools =
                        versions::list_github(&packages, &github_api, &token, 50).await?;
                    // An owner with an IP allow list refuses the Actions
                    // token, but its tags are still public over git.
                    let blocked: Vec<registry::Package> = packages
                        .iter()
                        .filter(|p| {
                            tools
                                .get(&versions::aqua_id(&p.name))
                                .and_then(|t| t.error.as_deref())
                                .is_some_and(versions::is_ip_allow_list)
                        })
                        .cloned()
                        .collect();
                    if !blocked.is_empty() {
                        for (id, listing) in versions::list_git(&blocked, 16).await? {
                            if listing.error.is_none() {
                                tools.insert(id, listing);
                            }
                        }
                    }
                    tools
                }
                "git" => versions::list_git(&packages, 16).await?,
                other => bail!("unknown source {other}"),
            };
            for (provider, core) in &cfg.index.core {
                let id = format!("core:{provider}");
                if !only.is_empty() && !only.contains(&id) {
                    continue;
                }
                let listing = match provider.as_str() {
                    "java" => versions::list_adoptium(&adoptium, core.max_versions)
                        .await
                        .unwrap_or_else(|e| versions::ToolListing {
                            source: "adoptium".into(),
                            versions: Vec::new(),
                            error: Some(format!("{e:#}")),
                        }),
                    other => bail!("no lister for core provider {other}"),
                };
                tools.insert(id, listing);
            }
            let listing = versions::Listing {
                listed_at: Timestamp::now().to_string(),
                tools,
            };
            let path = work.join(out);
            write_json(&path, &listing)?;
            let failed = listing.tools.values().filter(|t| t.error.is_some()).count();
            eprintln!(
                "listed {} tools ({failed} failed) into {}",
                listing.tools.len(),
                path.display()
            );
        }
        Command::Build {
            config,
            work,
            listing,
            previous,
            oyzu,
            no_verify,
            attempts,
            engine_rev,
            adoptium,
            only,
            now,
            out,
        } => {
            let cfg = config::Config::load(&config)?;
            let attempts = attempts::Attempts::read(&attempts)?;
            let listing: versions::Listing = serde_json::from_slice(&std::fs::read(&listing)?)?;
            let previous = previous.map(|p| index::Index::read(&p)).transpose()?;
            let (index, report, attempts) = build::build(build::Inputs {
                config: &cfg,
                registry_path: work.join("registry.yaml"),
                listing,
                previous,
                now: now.unwrap_or_else(Timestamp::now),
                oyzu: (!no_verify).then_some(resolve::Oyzu { binary: oyzu }),
                only: (!only.is_empty()).then(|| only.into_iter().collect()),
                work: work.join("records"),
                attempts,
                engine_rev,
            })?;
            let (mut index, mut report) = (index, report);
            if !no_verify {
                // Copyleft tools served from Oyzu's host need their source
                // archive beside each version (none until a person reviews one).
                let client = reqwest::Client::builder()
                    .user_agent("oyzu-tool-index")
                    .build()?;
                let mut found = Vec::new();
                for want in sources::wanted(&index, &cfg) {
                    let got = match sources::locate(&client, &adoptium, &want).await {
                        Ok(url) => sources::fetch(&client, &url).await,
                        Err(e) => Err(e),
                    };
                    match got {
                        Ok(source) => found.push((want, source)),
                        Err(e) => report
                            .source_failures
                            .push(format!("{} {}: {e:#}", want.tool, want.version)),
                    }
                }
                report.sources_recorded = sources::apply(&mut index, &found);
                index.check()?;
            }
            std::fs::create_dir_all(&out)?;
            std::fs::write(out.join("index.json"), index.to_bytes()?)?;
            std::fs::write(out.join("attempts.json"), attempts.to_bytes()?)?;
            std::fs::copy(work.join("NOTICE"), out.join("NOTICE")).context("run `fetch` first")?;
            print_json(&report)?;
        }
        Command::Review {
            previous,
            next,
            markdown,
        } => {
            let previous = previous.map(|p| index::Index::read(&p)).transpose()?;
            let next = index::Index::read(&next)?;
            // The committed index stays unstamped; `stamp` refuses anything else.
            if next.stamped() {
                bail!("the index under review carries serial or expires; only release copies are stamped");
            }
            let r = review::review(previous.as_ref(), &next);
            if let Some(path) = markdown {
                std::fs::write(path, review::markdown(&r))?;
            }
            print_json(&r)?;
        }
    }
    Ok(())
}
