//! Builds, reviews, stamps and checks Oyzu's public tool index for release. The
//! `tool-index` binary is the job's command line; see README.md.
pub mod attempts;
pub mod build;
pub mod config;
pub mod fetch;
pub mod index;
pub mod names;
pub mod registry;
pub mod release;
pub mod resolve;
pub mod review;
pub mod serial;
pub mod serving;
pub mod stamp;
pub mod versions;
