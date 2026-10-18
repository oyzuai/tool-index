//! The signed plugin catalog (`plugins.json`), published beside `index.json`
//! in the same release (Plugins S3a; tool-index#36).
//!
//! The contract is owned by oyzu: `docs/contracts/plugins-v1alpha1/
//! catalog.schema.json` and the semantic rules of `crates/oyzu-plugin-protocol/
//! src/catalog.rs`. Private oyzu cannot be a dependency of the release tool
//! (it builds from crates.io only, so the daily re-sign survives a broken oyzu
//! read App), so the types and the rules the schema cannot express are ported
//! here, and the tests run the canonical examples and every catalog invalid
//! case against the port (`tests/fixtures/plugins-v1alpha1`, pinned to an oyzu
//! commit). The port stays in step with oyzu by refreshing those fixtures.
//!
//! The source of truth is `config/plugins.toml`, the catalog's `content` in
//! TOML. [`generate`] checks it, keeps only the releases a verifier found
//! available (catalog-last), stamps the envelope and checks the result again.
use anyhow::{anyhow, ensure, Context, Result};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_VALIDITY_SECONDS: i64 = 31 * 24 * 3600;
pub const MAX_MATCHER_NODES: usize = 32;
pub const MAX_MATCHER_DEPTH: usize = 4;
/// The official publisher. Its packages, and only its packages, are routed
/// from `ghcr.io/oyzuai/plugins/<name>`.
pub const OFFICIAL_PUBLISHER: &str = "oyzu";
const OFFICIAL_REPOSITORY: &str = "ghcr.io/oyzuai/plugins/";

// ---------------------------------------------------------------- types

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: SchemaVersion,
    #[serde(rename = "type")]
    pub kind: CatalogType,
    pub serial: u64,
    pub generated_at: String,
    pub expires_at: String,
    pub content_digest: String,
    pub content: CatalogContent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaVersion {
    #[serde(rename = "v1alpha1")]
    V1alpha1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogType {
    #[serde(rename = "plugin-catalog")]
    PluginCatalog,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogContent {
    pub authority: String,
    pub packages: Vec<Package>,
    pub references: Vec<Reference>,
    pub detections: Vec<Detection>,
    pub withdrawals: Vec<Withdrawal>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub publisher: String,
    pub name: String,
    pub maintainer: String,
    /// Oldest first by semantic version precedence.
    pub releases: Vec<Release>,
}

impl Package {
    /// `<publisher>/<name>`, as `oyzu.lock` records it.
    pub fn id(&self) -> String {
        format!("{}/{}", self.publisher, self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub major: u32,
    pub preview: bool,
    pub protocol_generations: Vec<SchemaVersion>,
    pub roles: Vec<ReleaseRole>,
    pub profiles: Vec<Profile>,
    pub root_digest: String,
    pub platforms: Vec<PlatformPayload>,
    pub routes: Vec<Route>,
    pub qualification: Qualification,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Builder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRole {
    pub role: Role,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub role: Role,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Os {
    Linux,
    Darwin,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Arch {
    Amd64,
    Arm64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Abi {
    Gnu,
    Musl,
    Msvc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformPayload {
    pub os: Os,
    pub arch: Arch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<Abi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_platform: Option<String>,
    pub manifest_digest: String,
    pub payload_digest: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RouteWire", into = "RouteWire")]
pub enum Route {
    /// `<registry>/<repository>@sha256:<root digest>`.
    Oci(String),
    /// An HTTPS OCI image layout archive of the release graph.
    Https(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteWire {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

impl TryFrom<RouteWire> for Route {
    type Error = anyhow::Error;
    fn try_from(wire: RouteWire) -> Result<Self> {
        match (wire.kind.as_str(), wire.reference, wire.url) {
            ("oci", Some(reference), None) => Ok(Self::Oci(reference)),
            ("https", None, Some(url)) => Ok(Self::Https(url)),
            _ => Err(invalid(
                "routes",
                "must be an oci reference or an https url",
            )),
        }
    }
}

impl From<Route> for RouteWire {
    fn from(route: Route) -> Self {
        match route {
            Route::Oci(reference) => Self {
                kind: "oci".into(),
                reference: Some(reference),
                url: None,
            },
            Route::Https(url) => Self {
                kind: "https".into(),
                reference: None,
                url: Some(url),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Qualification {
    pub tested: Vec<Tested>,
    pub support: Support,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tested {
    pub core: String,
    pub platforms: Vec<PlatformKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformKey {
    pub os: Os,
    pub arch: Arch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<Abi>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Support {
    pub status: SupportStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportStatus {
    Supported,
    Deprecated,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub reference: String,
    pub package: String,
    pub profile: String,
    pub role: Role,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detection {
    pub id: String,
    pub uses: String,
    #[serde(rename = "match")]
    pub matcher: Matcher,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "MatcherWire", into = "MatcherWire")]
pub enum Matcher {
    File(String),
    Field(FieldPredicate),
    All(Vec<Matcher>),
    Any(Vec<Matcher>),
    Not(Box<Matcher>),
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatcherWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field: Option<FieldPredicate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    all: Option<Vec<Matcher>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    any: Option<Vec<Matcher>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    not: Option<Box<Matcher>>,
}

impl TryFrom<MatcherWire> for Matcher {
    type Error = anyhow::Error;
    fn try_from(w: MatcherWire) -> Result<Self> {
        match (w.file, w.field, w.all, w.any, w.not) {
            (Some(file), None, None, None, None) => Ok(Self::File(file)),
            (None, Some(field), None, None, None) => Ok(Self::Field(field)),
            (None, None, Some(all), None, None) => Ok(Self::All(all)),
            (None, None, None, Some(any), None) => Ok(Self::Any(any)),
            (None, None, None, None, Some(not)) => Ok(Self::Not(not)),
            _ => Err(invalid(
                "detections.match",
                "must hold exactly one of file, field, all, any or not",
            )),
        }
    }
}

impl From<Matcher> for MatcherWire {
    fn from(matcher: Matcher) -> Self {
        let mut w = Self::default();
        match matcher {
            Matcher::File(file) => w.file = Some(file),
            Matcher::Field(field) => w.field = Some(field),
            Matcher::All(all) => w.all = Some(all),
            Matcher::Any(any) => w.any = Some(any),
            Matcher::Not(not) => w.not = Some(not),
        }
        w
    }
}

impl Matcher {
    fn nodes(&self) -> usize {
        match self {
            Self::File(_) | Self::Field(_) => 1,
            Self::All(items) | Self::Any(items) => 1 + items.iter().map(Self::nodes).sum::<usize>(),
            Self::Not(inner) => 1 + inner.nodes(),
        }
    }

    fn depth(&self) -> usize {
        match self {
            Self::File(_) | Self::Field(_) => 1,
            Self::All(items) | Self::Any(items) => {
                1 + items.iter().map(Self::depth).max().unwrap_or(0)
            }
            Self::Not(inner) => 1 + inner.depth(),
        }
    }

    /// Every way of matching needs a positive observation; an exclusion
    /// alone would match any directory.
    fn positive(&self) -> bool {
        match self {
            Self::File(_) | Self::Field(_) => true,
            Self::All(items) => items.iter().any(Self::positive),
            Self::Any(items) => items.iter().all(Self::positive),
            Self::Not(_) => false,
        }
    }

    fn check(&self) -> Result<()> {
        match self {
            Self::File(file) => filename("detections.match.file", file),
            Self::Field(field) => field.check(),
            Self::All(items) | Self::Any(items) => {
                count("detections.match", items, 2, 16)?;
                items.iter().try_for_each(Self::check)
            }
            Self::Not(inner) if matches!(**inner, Self::Not(_)) => Err(invalid(
                "detections.match",
                "a not directly inside a not is refused; write the inner matcher",
            )),
            Self::Not(inner) => inner.check(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FieldWire", into = "FieldWire")]
pub struct FieldPredicate {
    pub file: String,
    pub format: FieldFormat,
    pub path: Vec<String>,
    pub test: FieldTest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldFormat {
    Json,
    Toml,
    /// The single-valued directives `module`, `go` and `toolchain`.
    GoMod,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldTest {
    Present,
    Equals(String),
    Prefix(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldWire {
    file: String,
    format: FieldFormat,
    path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    present: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    equals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefix: Option<String>,
}

impl TryFrom<FieldWire> for FieldPredicate {
    type Error = anyhow::Error;
    fn try_from(w: FieldWire) -> Result<Self> {
        let test = match (w.present, w.equals, w.prefix) {
            (Some(true), None, None) => FieldTest::Present,
            (None, Some(value), None) => FieldTest::Equals(value),
            (None, None, Some(value)) => FieldTest::Prefix(value),
            _ => {
                return Err(invalid(
                    "detections.match.field",
                    "needs exactly one test: present true, equals or prefix",
                ))
            }
        };
        Ok(Self {
            file: w.file,
            format: w.format,
            path: w.path,
            test,
        })
    }
}

impl From<FieldPredicate> for FieldWire {
    fn from(f: FieldPredicate) -> Self {
        let (present, equals, prefix) = match f.test {
            FieldTest::Present => (Some(true), None, None),
            FieldTest::Equals(v) => (None, Some(v), None),
            FieldTest::Prefix(v) => (None, None, Some(v)),
        };
        Self {
            file: f.file,
            format: f.format,
            path: f.path,
            present,
            equals,
            prefix,
        }
    }
}

impl FieldPredicate {
    fn check(&self) -> Result<()> {
        let field = "detections.match.field";
        filename(&format!("{field}.file"), &self.file)?;
        count(&format!("{field}.path"), &self.path, 1, 8)?;
        for key in &self.path {
            let ok = (1..=128).contains(&key.len())
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'@' | b'.' | b'-'));
            ensure_rule(
                ok,
                &format!("{field}.path"),
                "keys must match [A-Za-z0-9_@.-]{1,128}",
            )?;
        }
        if self.format == FieldFormat::GoMod
            && !matches!(
                self.path.as_slice(),
                [key] if matches!(key.as_str(), "module" | "go" | "toolchain")
            )
        {
            return Err(invalid(
                &format!("{field}.path"),
                "go-mod addresses only the module, go or toolchain directive",
            ));
        }
        match &self.test {
            FieldTest::Present => Ok(()),
            FieldTest::Equals(v) => bounded(&format!("{field}.equals"), v, 0, 256),
            FieldTest::Prefix(v) => bounded(&format!("{field}.prefix"), v, 1, 256),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Withdrawal {
    pub package: String,
    pub version: String,
    pub root_digest: String,
    pub kind: WithdrawalKind,
    pub at: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WithdrawalKind {
    Withdrawn,
    Revoked,
}

// ---------------------------------------------------------------- checks
//
// Ported from oyzu-plugin-protocol's `check.rs` and `catalog.rs`; each helper
// keeps the name and rule of the original.

fn invalid(field: &str, rule: &str) -> anyhow::Error {
    anyhow!("invalid plugin catalog: {field} {rule}")
}

fn ensure_rule(ok: bool, field: &str, rule: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(field, rule))
    }
}

fn lower_digit(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit()
}

fn shaped(value: &str, max: usize, first: fn(u8) -> bool, rest: fn(u8) -> bool) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= max
        && first(bytes[0])
        && bytes[1..].iter().all(|&b| rest(b))
}

/// `[a-z0-9][a-z0-9-]{0,63}`.
fn is_component(value: &str) -> bool {
    shaped(value, 64, lower_digit, |b| lower_digit(b) || b == b'-')
}

fn component(field: &str, value: &str) -> Result<()> {
    ensure_rule(
        is_component(value),
        field,
        "must match [a-z0-9][a-z0-9-]{0,63}",
    )
}

fn feature(field: &str, value: &str) -> Result<()> {
    let ok = shaped(value, 64, lower_digit, |b| {
        lower_digit(b) || matches!(b, b'.' | b'-')
    });
    ensure_rule(ok, field, "must match [a-z0-9][a-z0-9.-]{0,63}")
}

fn message(field: &str, value: &str) -> Result<()> {
    ensure_rule(
        (1..=4000).contains(&value.chars().count()),
        field,
        "must hold 1 to 4000 characters",
    )
}

fn bounded(field: &str, value: &str, min: usize, max: usize) -> Result<()> {
    ensure_rule(
        (min..=max).contains(&value.chars().count()),
        field,
        "has an invalid length",
    )
}

fn count<T>(field: &str, values: &[T], min: usize, max: usize) -> Result<()> {
    ensure_rule(
        (min..=max).contains(&values.len()),
        field,
        "has an invalid number of items",
    )
}

fn unique<T: Ord>(field: &str, values: &[T]) -> Result<()> {
    let mut seen = BTreeSet::new();
    ensure_rule(
        values.iter().all(|v| seen.insert(v)),
        field,
        "items must be unique",
    )
}

fn digest(field: &str, value: &str) -> Result<()> {
    let ok = value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    ensure_rule(ok, field, "must be sha256:<64 lowercase hex>")
}

fn filename(field: &str, value: &str) -> Result<()> {
    let edge = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'@' | b'+' | b'-');
    let bytes = value.as_bytes();
    let ok = !bytes.is_empty()
        && bytes.len() <= 255
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes.iter().all(|&b| edge(b) || matches!(b, b'.' | b' '));
    ensure_rule(ok, field, "must be one portable file name")
}

fn numeric(part: &str) -> bool {
    !part.is_empty()
        && part.bytes().all(|b| b.is_ascii_digit())
        && (part == "0" || !part.starts_with('0'))
}

fn dotted_ident(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

fn semver(field: &str, value: &str) -> Result<()> {
    let (rest, build) = value
        .split_once('+')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    let (core, pre) = rest
        .split_once('-')
        .map_or((rest, None), |(a, b)| (a, Some(b)));
    let parts: Vec<_> = core.split('.').collect();
    let ok = value.len() <= 128
        && parts.len() == 3
        && parts.iter().all(|p| numeric(p))
        && pre.is_none_or(dotted_ident)
        && build.is_none_or(dotted_ident);
    ensure_rule(ok, field, "must be a semantic version")
}

fn two_components(field: &str, value: &str, rule: &str) -> Result<()> {
    match value.split_once('/') {
        Some((a, b)) if is_component(a) && is_component(b) => Ok(()),
        _ => Err(invalid(field, rule)),
    }
}

fn short(field: &str, value: &str) -> Result<()> {
    two_components(
        field,
        value,
        "must be a logical reference <package>/<profile>",
    )
}

fn package_id(field: &str, value: &str) -> Result<()> {
    two_components(
        field,
        value,
        "must be a package identity <publisher>/<name>",
    )
}

/// Strictly ascending: sorted and unique under `order`.
fn ascending<T>(field: &str, values: &[T], order: impl Fn(&T, &T) -> Ordering) -> Result<()> {
    ensure_rule(
        values
            .windows(2)
            .all(|p| order(&p[0], &p[1]) == Ordering::Less),
        field,
        "must be sorted and unique",
    )
}

fn split_version(version: &str) -> (Vec<u64>, Option<&str>) {
    let version = version.split_once('+').map_or(version, |(a, _)| a);
    let (core, pre) = version
        .split_once('-')
        .map_or((version, None), |(a, b)| (a, Some(b)));
    let numbers = core
        .split('.')
        .map(|part| part.parse().unwrap_or(u64::MAX))
        .collect();
    (numbers, pre)
}

/// Semantic version precedence (build metadata ignored).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (a_core, a_pre) = split_version(a);
    let (b_core, b_pre) = split_version(b);
    a_core.cmp(&b_core).then_with(|| match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => {
            let mut left = a.split('.');
            let mut right = b.split('.');
            loop {
                match (left.next(), right.next()) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(x), Some(y)) => {
                        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                            (Ok(x), Ok(y)) => x.cmp(&y),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => x.cmp(y),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                }
            }
        }
    })
}

/// Seconds since the epoch of `YYYY-MM-DDTHH:MM:SSZ`, the only accepted
/// spelling; `None` for anything else, including impossible dates.
pub fn timestamp(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    let shape = bytes.len() == 20
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            4 | 7 => b == b'-',
            10 => b == b'T',
            13 | 16 => b == b':',
            19 => b == b'Z',
            _ => b.is_ascii_digit(),
        });
    if !shape || value[17..19].parse::<u8>().ok()? > 59 {
        return None;
    }
    value.parse::<Timestamp>().ok().map(|t| t.as_second())
}

fn host_ok(host: &str, dotted: bool) -> bool {
    let (name, port) = host
        .split_once(':')
        .map_or((host, None), |(a, b)| (a, Some(b)));
    let labels: Vec<_> = name.split('.').collect();
    let label_ok = |label: &&str| {
        let bytes = label.as_bytes();
        (1..=63).contains(&bytes.len())
            && lower_digit(bytes[0])
            && lower_digit(bytes[bytes.len() - 1])
            && bytes.iter().all(|&b| lower_digit(b) || b == b'-')
    };
    (!dotted || labels.len() >= 2)
        && labels.iter().all(label_ok)
        && port.is_none_or(|p| (1..=5).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
}

fn check_https(url: &str) -> Result<()> {
    let ok = url.len() <= 2048
        && url.strip_prefix("https://").is_some_and(|rest| {
            rest.split_once('/').is_some_and(|(host, path)| {
                host_ok(host, true)
                    && path.split('/').all(|segment| {
                        !segment.is_empty()
                            && segment
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"._~%+-".contains(&b))
                    })
            })
        });
    ensure_rule(
        ok,
        "routes.url",
        "must be an https URL without credentials, query or fragment",
    )
}

fn platform_key(os: Os, arch: Arch, abi: Option<Abi>) -> (String, String, String) {
    let name = |value: Value| value.as_str().unwrap_or_default().to_owned();
    (
        name(serde_json::to_value(os).unwrap_or_default()),
        name(serde_json::to_value(arch).unwrap_or_default()),
        abi.map(|a| name(serde_json::to_value(a).unwrap_or_default()))
            .unwrap_or_default(),
    )
}

/// `sha256:` of the RFC 8785 canonical JSON of `content`.
pub fn content_digest(content: &CatalogContent) -> Result<String> {
    canonical_digest(&serde_json::to_value(content)?)
}

/// `sha256:` of the RFC 8785 canonical JSON of any catalog value.
pub fn canonical_digest(value: &Value) -> Result<String> {
    let mut canonical = Vec::new();
    write_canonical(value, &mut canonical)?;
    Ok(sha256(&canonical))
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// RFC 8785 for the values a catalog holds: strings, integers, booleans,
/// arrays and objects. Keys sort by UTF-16 code unit; serde_json's string
/// escaping matches the RFC's.
fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
            out.push(b'{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, key)?;
                out.push(b':');
                write_canonical(item, out)?;
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Number(n) if !n.is_f64() => serde_json::to_writer(&mut *out, n)?,
        Value::Number(_) => return Err(invalid("content", "holds a non-integer number")),
        scalar => serde_json::to_writer(&mut *out, scalar)?,
    }
    Ok(())
}

impl Catalog {
    /// Decode and check one catalog document (the size limit applies before
    /// parsing). Duplicate JSON keys are not detected here; the generator
    /// writes the document itself.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_CATALOG_BYTES,
            "the plugin catalog is larger than {MAX_CATALOG_BYTES} bytes"
        );
        let value: Value = serde_json::from_slice(bytes).context("parsing the plugin catalog")?;
        let catalog: Self =
            serde_json::from_value(value.clone()).context("decoding the plugin catalog")?;
        // The digest covers the content as signed, not a re-serialization.
        catalog.check_against(&canonical_digest(&value["content"])?)?;
        Ok(catalog)
    }

    /// Checks a typed catalog, computing the digest from its typed content.
    pub fn check(&self) -> Result<()> {
        self.check_against(&content_digest(&self.content)?)
    }

    fn check_against(&self, computed: &str) -> Result<()> {
        ensure_rule(
            (1..=(1u64 << 53) - 1).contains(&self.serial),
            "serial",
            "must be between 1 and 2^53-1",
        )?;
        let generated = timestamp(&self.generated_at)
            .ok_or_else(|| invalid("generated_at", "must be an RFC 3339 UTC time in seconds"))?;
        let expires = timestamp(&self.expires_at)
            .ok_or_else(|| invalid("expires_at", "must be an RFC 3339 UTC time in seconds"))?;
        ensure_rule(
            expires > generated && expires - generated <= MAX_VALIDITY_SECONDS,
            "expires_at",
            "must be after generated_at and at most 31 days later",
        )?;
        digest("content_digest", &self.content_digest)?;
        ensure_rule(
            self.content_digest == computed,
            "content_digest",
            "does not match the canonical JSON of content",
        )?;
        self.content.check()
    }
}

impl CatalogContent {
    pub fn check(&self) -> Result<()> {
        component("content.authority", &self.authority)?;
        count("packages", &self.packages, 0, 1024)?;
        count("references", &self.references, 0, 4096)?;
        count("detections", &self.detections, 0, 4096)?;
        count("withdrawals", &self.withdrawals, 0, 4096)?;
        for package in &self.packages {
            package.check()?;
        }
        let ids: Vec<_> = self.packages.iter().map(Package::id).collect();
        ascending("packages", &ids, |a, b| a.cmp(b))?;

        for reference in &self.references {
            short("references.reference", &reference.reference)?;
            package_id("references.package", &reference.package)?;
            component("references.profile", &reference.profile)?;
            bounded("references.description", &reference.description, 1, 512)?;
            let package = self
                .packages
                .iter()
                .find(|p| p.id() == reference.package)
                .ok_or_else(|| invalid("references.package", "is not listed under packages"))?;
            let registered = package.releases.iter().any(|r| {
                r.profiles
                    .iter()
                    .any(|p| p.name == reference.profile && p.role == reference.role)
            });
            ensure_rule(
                registered,
                "references.profile",
                "no release of the package registers this profile and role",
            )?;
        }
        let names: Vec<_> = self.references.iter().map(|r| &r.reference).collect();
        ascending("references", &names, |a, b| a.cmp(b))?;

        for detection in &self.detections {
            component("detections.id", &detection.id)?;
            short("detections.uses", &detection.uses)?;
            ensure_rule(
                self.references
                    .iter()
                    .any(|r| r.reference == detection.uses),
                "detections.uses",
                "is not listed under references",
            )?;
            let matcher = &detection.matcher;
            matcher.check()?;
            ensure_rule(
                matcher.nodes() <= MAX_MATCHER_NODES && matcher.depth() <= MAX_MATCHER_DEPTH,
                "detections.match",
                "exceeds 32 nodes or a nesting depth of 4",
            )?;
            ensure_rule(
                matcher.positive(),
                "detections.match",
                "must require a positive observation; an exclusion alone matches nothing",
            )?;
        }
        let ids: Vec<_> = self.detections.iter().map(|d| &d.id).collect();
        ascending("detections", &ids, |a, b| a.cmp(b))?;

        for w in &self.withdrawals {
            package_id("withdrawals.package", &w.package)?;
            semver("withdrawals.version", &w.version)?;
            digest("withdrawals.root_digest", &w.root_digest)?;
            ensure_rule(
                timestamp(&w.at).is_some(),
                "withdrawals.at",
                "must be an RFC 3339 UTC time",
            )?;
            message("withdrawals.reason", &w.reason)?;
            let releases = self
                .packages
                .iter()
                .find(|p| p.id() == w.package)
                .map_or(&[][..], |p| &p.releases);
            let listed = |version: &str| {
                releases
                    .iter()
                    .any(|r| compare_versions(&r.version, version) == Ordering::Equal)
            };
            ensure_rule(
                !listed(&w.version),
                "withdrawals.version",
                "a withdrawn release is not also listed under releases",
            )?;
            if let Some(replacement) = &w.replacement {
                semver("withdrawals.replacement", replacement)?;
                ensure_rule(
                    listed(replacement),
                    "withdrawals.replacement",
                    "must be a listed release of the package",
                )?;
            }
        }
        let keys: Vec<_> = self
            .withdrawals
            .iter()
            .map(|w| (&w.package, &w.version))
            .collect();
        ascending("withdrawals", &keys, |a, b| {
            a.0.cmp(b.0).then_with(|| compare_versions(a.1, b.1))
        })?;

        // The lock matches a selection by root digest, so one root names one
        // release of a package, listed or withdrawn.
        let mut roots: Vec<(String, &str)> = self
            .packages
            .iter()
            .flat_map(|p| {
                let id = p.id();
                p.releases
                    .iter()
                    .map(move |r| (id.clone(), r.root_digest.as_str()))
            })
            .collect();
        roots.extend(
            self.withdrawals
                .iter()
                .map(|w| (w.package.clone(), w.root_digest.as_str())),
        );
        roots.sort();
        ensure_rule(
            !roots.windows(2).any(|pair| pair[0] == pair[1]),
            "root_digest",
            "names more than one release of a package",
        )
    }
}

impl Package {
    fn check(&self) -> Result<()> {
        component("packages.publisher", &self.publisher)?;
        component("packages.name", &self.name)?;
        bounded("packages.maintainer", &self.maintainer, 1, 256)?;
        count("packages.releases", &self.releases, 1, 256)?;
        for release in &self.releases {
            release.check(self)?;
        }
        let versions: Vec<_> = self.releases.iter().map(|r| r.version.as_str()).collect();
        ascending("packages.releases", &versions, |a, b| {
            compare_versions(a, b)
        })?;
        Ok(())
    }
}

impl Release {
    fn check(&self, package: &Package) -> Result<()> {
        semver("releases.version", &self.version)?;
        let (core, prerelease) = split_version(&self.version);
        ensure_rule(
            core.first().copied() == Some(u64::from(self.major)) && self.major <= 999_999_999,
            "releases.major",
            "must equal the version's major",
        )?;
        ensure_rule(
            prerelease.is_none() || self.preview,
            "releases.preview",
            "a semantic-version prerelease is always a preview",
        )?;
        count(
            "releases.protocol_generations",
            &self.protocol_generations,
            1,
            16,
        )?;
        ensure_rule(
            {
                let mut seen = BTreeSet::new();
                self.protocol_generations
                    .iter()
                    .all(|g| seen.insert(format!("{g:?}")))
            },
            "releases.protocol_generations",
            "items must be unique",
        )?;
        count("releases.roles", &self.roles, 1, 16)?;
        for role in &self.roles {
            count("releases.roles.features", &role.features, 0, 64)?;
            for f in &role.features {
                feature("releases.roles.features", f)?;
            }
            ascending("releases.roles.features", &role.features, |a, b| a.cmp(b))?;
        }
        let roles: Vec<_> = self.roles.iter().map(|r| r.role).collect();
        ascending("releases.roles", &roles, |a, b| a.cmp(b))?;
        count("releases.profiles", &self.profiles, 1, 64)?;
        for profile in &self.profiles {
            component("releases.profiles.name", &profile.name)?;
        }
        let profiles: Vec<_> = self.profiles.iter().map(|p| &p.name).collect();
        ascending("releases.profiles", &profiles, |a, b| a.cmp(b))?;
        digest("releases.root_digest", &self.root_digest)?;

        count("releases.platforms", &self.platforms, 1, 32)?;
        for platform in &self.platforms {
            if let Some(min) = &platform.min_platform {
                let ok = (1..=4).contains(&min.split('.').count())
                    && min.split('.').all(|part| {
                        (1..=5).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit())
                    });
                ensure_rule(ok, "platforms.min_platform", "must be dotted digits")?;
            }
            digest("platforms.manifest_digest", &platform.manifest_digest)?;
            digest("platforms.payload_digest", &platform.payload_digest)?;
            ensure_rule(
                (1..=(1 << 30)).contains(&platform.size),
                "platforms.size",
                "must be 1 byte to 1 GiB",
            )?;
        }
        let shipped: Vec<_> = self
            .platforms
            .iter()
            .map(|p| platform_key(p.os, p.arch, p.abi))
            .collect();
        ascending("releases.platforms", &shipped, |a, b| a.cmp(b))?;

        count("releases.routes", &self.routes, 1, 2)?;
        let kinds: Vec<_> = self
            .routes
            .iter()
            .map(|r| matches!(r, Route::Https(_)))
            .collect();
        unique("releases.routes", &kinds)?;
        for route in &self.routes {
            match route {
                Route::Oci(reference) => self.check_oci(package, reference)?,
                Route::Https(url) => check_https(url)?,
            }
        }

        let q = &self.qualification;
        count("qualification.tested", &q.tested, 0, 64)?;
        for tested in &q.tested {
            semver("qualification.tested.core", &tested.core)?;
            count("qualification.tested.platforms", &tested.platforms, 1, 32)?;
            let keys: Vec<_> = tested
                .platforms
                .iter()
                .map(|p| platform_key(p.os, p.arch, p.abi))
                .collect();
            ascending("qualification.tested.platforms", &keys, |a, b| a.cmp(b))?;
            ensure_rule(
                keys.iter().all(|k| shipped.contains(k)),
                "qualification.tested.platforms",
                "names a platform the release does not ship",
            )?;
        }
        let cores: Vec<_> = q.tested.iter().map(|t| t.core.as_str()).collect();
        ascending("qualification.tested", &cores, |a, b| {
            compare_versions(a, b)
        })?;
        if let Some(ends) = &q.support.ends {
            ensure_rule(
                timestamp(ends).is_some(),
                "support.ends",
                "must be an RFC 3339 UTC time",
            )?;
        }
        if let Some(m) = &q.support.message {
            message("support.message", m)?;
        }
        ensure_rule(
            q.support.status != SupportStatus::Deprecated
                || (q.support.ends.is_some() && q.support.message.is_some()),
            "support",
            "a deprecated release states its end and a message",
        )
    }

    fn check_oci(&self, package: &Package, reference: &str) -> Result<()> {
        let field = "routes.reference";
        let (location, digest_text) = reference
            .split_once('@')
            .ok_or_else(|| invalid(field, "must be pinned by digest"))?;
        digest(field, digest_text)?;
        let (registry, repository) = location
            .split_once('/')
            .ok_or_else(|| invalid(field, "must name a registry and repository"))?;
        let repository_ok = repository.split('/').all(|part| {
            !part.is_empty()
                && part
                    .split(['.', '_', '-'])
                    .all(|piece| !piece.is_empty() && piece.bytes().all(lower_digit))
        });
        ensure_rule(
            reference.len() <= 512 && host_ok(registry, false) && repository_ok,
            field,
            "must be <registry>/<repository>@sha256:<hex>",
        )?;
        ensure_rule(
            digest_text == self.root_digest,
            field,
            "must address the release root digest",
        )?;
        // The official repository holds the official publisher's packages only.
        let ok = if package.publisher == OFFICIAL_PUBLISHER {
            location == format!("{OFFICIAL_REPOSITORY}{}", package.name)
        } else {
            !location.starts_with(OFFICIAL_REPOSITORY)
        };
        ensure_rule(
            ok,
            field,
            "packages of publisher oyzu, and only those, are routed from ghcr.io/oyzuai/plugins/<name>",
        )
    }
}

// ------------------------------------------------------------ generation

/// `config/plugins.toml`: the catalog's `content` in TOML, reviewed like a
/// tool change. Every list may be omitted when empty.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub authority: String,
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub references: Vec<Reference>,
    #[serde(default)]
    pub detections: Vec<Detection>,
    #[serde(default)]
    pub withdrawals: Vec<Withdrawal>,
}

impl Source {
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).context("parsing the plugin source")
    }

    pub fn content(self) -> CatalogContent {
        CatalogContent {
            authority: self.authority,
            packages: self.packages,
            references: self.references,
            detections: self.detections,
            withdrawals: self.withdrawals,
        }
    }
}

/// Releases whose OCI release root and evidence a verifier found available
/// (`.github/scripts/plugin-availability.sh` writes this file).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    pub releases: Vec<VerifiedRelease>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedRelease {
    pub package: String,
    pub version: String,
    pub root_digest: String,
}

/// A release the verifier would have to confirm before it is advertised.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub package: String,
    pub name: String,
    pub version: String,
    pub root_digest: String,
    /// The OCI route without its digest (`ghcr.io/oyzuai/plugins/go`), if any.
    pub repository: Option<String>,
}

pub fn candidates(content: &CatalogContent) -> Vec<Candidate> {
    content
        .packages
        .iter()
        .flat_map(|p| {
            p.releases.iter().map(|r| Candidate {
                package: p.id(),
                name: p.name.clone(),
                version: r.version.clone(),
                root_digest: r.root_digest.clone(),
                repository: r.routes.iter().find_map(|route| match route {
                    Route::Oci(reference) => reference.split_once('@').map(|(l, _)| l.to_owned()),
                    Route::Https(_) => None,
                }),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Omitted {
    pub package: String,
    pub version: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct Generated {
    pub bytes: Vec<u8>,
    pub catalog: Catalog,
    /// Source releases left out because nothing verified them.
    pub omitted: Vec<Omitted>,
}

/// Catalog-last: keep only releases the verifier listed (same package,
/// version and root digest). A package left without releases goes, with its
/// references and the detections that use them. Withdrawals stay: they only
/// ever restrict, and a client must hear of them even when the bytes are gone;
/// an advisory replacement that was dropped is removed with it.
pub fn advertise(
    mut content: CatalogContent,
    verified: Option<&Verified>,
) -> (CatalogContent, Vec<Omitted>) {
    let mut omitted = Vec::new();
    for package in &mut content.packages {
        let id = package.id();
        package.releases.retain(|release| {
            let ok = verified.is_some_and(|v| {
                v.releases.iter().any(|x| {
                    x.package == id
                        && x.version == release.version
                        && x.root_digest == release.root_digest
                })
            });
            if !ok {
                omitted.push(Omitted {
                    package: id.clone(),
                    version: release.version.clone(),
                    reason: if verified.is_some() {
                        "release root or evidence not verified available".into()
                    } else {
                        "no availability verification was supplied".into()
                    },
                });
            }
            ok
        });
    }
    content.packages.retain(|p| !p.releases.is_empty());
    let kept: BTreeSet<_> = content.packages.iter().map(Package::id).collect();
    content.references.retain(|r| kept.contains(&r.package));
    let refs: BTreeSet<_> = content
        .references
        .iter()
        .map(|r| r.reference.clone())
        .collect();
    content.detections.retain(|d| refs.contains(&d.uses));
    for w in &mut content.withdrawals {
        let listed = content
            .packages
            .iter()
            .find(|p| p.id() == w.package)
            .is_some_and(|p| {
                w.replacement
                    .as_deref()
                    .is_some_and(|r| p.releases.iter().any(|x| x.version == r))
            });
        if !listed {
            w.replacement = None;
        }
    }
    (content, omitted)
}

/// Builds `plugins.json` for the release stamped `serial`.
///
/// `source` is checked in full first, so a mistake in an entry that is not yet
/// advertised still fails review. `expires_at` is the index's expiry, cut to
/// the 31-day client cap. The document is checked again as a client would.
pub fn generate(
    source: &str,
    verified: Option<&[u8]>,
    serial: u64,
    now: Timestamp,
    index_expires: Timestamp,
) -> Result<Generated> {
    let content = Source::parse(source)?.content();
    content.check().context("checking config/plugins.toml")?;
    let verified: Option<Verified> = verified
        .map(|b| serde_json::from_slice(b).context("parsing the availability file"))
        .transpose()?;
    let (content, omitted) = advertise(content, verified.as_ref());
    let now = Timestamp::from_second(now.as_second())?;
    let expires =
        index_expires.min(now.checked_add(SignedDuration::from_secs(MAX_VALIDITY_SECONDS))?);
    let catalog = Catalog {
        schema_version: SchemaVersion::V1alpha1,
        kind: CatalogType::PluginCatalog,
        serial,
        generated_at: now.to_string(),
        expires_at: Timestamp::from_second(expires.as_second())?.to_string(),
        content_digest: content_digest(&content)?,
        content,
    };
    let mut bytes = serde_json::to_vec_pretty(&catalog)?;
    bytes.push(b'\n');
    let decoded = Catalog::decode(&bytes).context("the generated catalog fails the contract")?;
    ensure!(
        decoded == catalog,
        "the generated catalog does not round-trip"
    );
    Ok(Generated {
        bytes,
        catalog,
        omitted,
    })
}

/// The release check: `plugins.json` decodes, belongs to the release
/// (`serial` equal to `index.json`'s) and has not expired at `now`.
pub fn check_release(bytes: &[u8], serial: u64, now: Timestamp) -> Result<Catalog> {
    let catalog = Catalog::decode(bytes)?;
    ensure!(
        catalog.serial == serial,
        "plugins.json has serial {}, but index.json has serial {serial}",
        catalog.serial
    );
    let expires = timestamp(&catalog.expires_at).context("plugins.json expires_at")?;
    ensure!(
        expires > now.as_second(),
        "plugins.json expired at {}",
        catalog.expires_at
    );
    Ok(catalog)
}

// ---------------------------------------------------------------- review

/// One reviewable difference between two versions of `config/plugins.toml`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Change {
    pub section: &'static str,
    pub key: String,
    pub kind: &'static str,
    /// Fields that differ, for a changed item.
    pub fields: Vec<String>,
    /// Touches what a plugin may detect, require, install from, or who signs
    /// and maintains it. Shown first in the review summary.
    pub sensitive: bool,
}

/// Fields whose change alters detector reach, requirements, source or signer.
const SENSITIVE_RELEASE: [&str; 6] = [
    "protocol_generations",
    "roles",
    "profiles",
    "root_digest",
    "platforms",
    "routes",
];

/// Compares two plugin sources structurally. Sensitive: any detection; any
/// added or changed reference; a package's authority, publisher or
/// maintainer; and, per release, roles/features, protocol generations,
/// profiles, root digest, platforms (including `min_platform`) and routes.
/// Qualification-only edits and withdrawals are listed but not sensitive.
pub fn review(previous: Option<&str>, next: &str) -> Result<Vec<Change>> {
    let old = match previous {
        Some(p) => Source::parse(p)?.content(),
        None => Source::parse("authority = \"oyzu\"")?.content(),
    };
    let new = Source::parse(next)?.content();
    let mut out = Vec::new();
    if old.authority != new.authority {
        out.push(Change {
            section: "authority",
            key: new.authority.clone(),
            kind: "changed",
            fields: vec!["authority".into()],
            sensitive: true,
        });
    }
    let map = |v: Vec<(String, Value)>| v.into_iter().collect::<BTreeMap<_, _>>();

    // Packages: identity fields, then releases keyed by version.
    let pk = |c: &CatalogContent| map(c.packages.iter().map(|p| (p.id(), p.header())).collect());
    diff("packages", &pk(&old), &pk(&new), |_| true, &mut out);
    for package in &new.packages {
        let id = package.id();
        let rel = |c: &CatalogContent| {
            map(c
                .packages
                .iter()
                .filter(|p| p.id() == id)
                .flat_map(|p| p.releases.iter())
                .map(|r| {
                    (
                        format!("{id}@{}", r.version),
                        serde_json::to_value(r).unwrap(),
                    )
                })
                .collect())
        };
        diff(
            "releases",
            &rel(&old),
            &rel(&new),
            |f| SENSITIVE_RELEASE.contains(&f),
            &mut out,
        );
    }
    for package in &old.packages {
        let id = package.id();
        if !new.packages.iter().any(|p| p.id() == id) {
            for r in &package.releases {
                out.push(Change {
                    section: "releases",
                    key: format!("{id}@{}", r.version),
                    kind: "removed",
                    fields: vec![],
                    sensitive: false,
                });
            }
        }
    }
    let refs = |c: &CatalogContent| {
        map(c
            .references
            .iter()
            .map(|r| (r.reference.clone(), serde_json::to_value(r).unwrap()))
            .collect())
    };
    diff("references", &refs(&old), &refs(&new), |_| true, &mut out);
    let dets = |c: &CatalogContent| {
        map(c
            .detections
            .iter()
            .map(|d| (d.id.clone(), serde_json::to_value(d).unwrap()))
            .collect())
    };
    diff("detections", &dets(&old), &dets(&new), |_| true, &mut out);
    let wds = |c: &CatalogContent| {
        map(c
            .withdrawals
            .iter()
            .map(|w| {
                (
                    format!("{}@{}", w.package, w.version),
                    serde_json::to_value(w).unwrap(),
                )
            })
            .collect())
    };
    diff("withdrawals", &wds(&old), &wds(&new), |_| false, &mut out);
    out.sort_by_key(|c| !c.sensitive);
    Ok(out)
}

impl Package {
    /// The identity fields review compares apart from releases.
    fn header(&self) -> Value {
        serde_json::json!({
            "publisher": self.publisher,
            "name": self.name,
            "maintainer": self.maintainer,
        })
    }
}

fn diff(
    section: &'static str,
    old: &BTreeMap<String, Value>,
    new: &BTreeMap<String, Value>,
    field_sensitive: impl Fn(&str) -> bool,
    out: &mut Vec<Change>,
) {
    let section_sensitive = matches!(section, "detections" | "references");
    for (key, value) in new {
        match old.get(key) {
            None => out.push(Change {
                section,
                key: key.clone(),
                kind: "added",
                fields: vec![],
                sensitive: section != "withdrawals",
            }),
            Some(before) if before != value => {
                let fields: Vec<String> = match (before, value) {
                    (Value::Object(a), Value::Object(b)) => a
                        .keys()
                        .chain(b.keys())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .filter(|k| a.get(*k) != b.get(*k))
                        .cloned()
                        .collect(),
                    _ => vec![],
                };
                let sensitive = section_sensitive || fields.iter().any(|f| field_sensitive(f));
                out.push(Change {
                    section,
                    key: key.clone(),
                    kind: "changed",
                    fields,
                    sensitive,
                });
            }
            Some(_) => {}
        }
    }
    for key in old.keys().filter(|k| !new.contains_key(*k)) {
        out.push(Change {
            section,
            key: key.clone(),
            kind: "removed",
            fields: vec![],
            sensitive: section == "detections",
        });
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
