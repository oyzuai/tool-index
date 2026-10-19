# Tool index job

Builds, reviews and releases Oyzu's public tool index: the signed
metadata (versions, release dates, download pointers, SHA-256 digests and
serving classes) that the oyzu CLI reads instead of upstream feeds.

Status: **live.** The daily job publishes signed releases `index-v<serial>`
on this repository, and the oyzu CLI reads the latest by default. `index-v1`
was signed with an in-toto v0.1 statement the CLI refuses; every later release
is accepted. Each release expires 14 days after signing, and `release.yml`
re-signs daily.

## What a run does

| Step | Command | Network |
| --- | --- | --- |
| Fetch the pinned aqua registry snapshot, its license (the `NOTICE`) and the mise name catalog, each checked against its pinned digest | `tool-index fetch` | yes |
| List upstream versions: GitHub GraphQL in production (`git ls-remote` for owners whose IP allow list refuses the token), `git ls-remote` without a token; for the core providers, Adoptium, nodejs.org, go.dev, python-build-standalone, archive.apache.org (Maven), services.gradle.org and the npm registry, and `git ls-remote` of the publisher's tags for Helm, kubectl, k3d, Liquibase, Tailscale and Rust | `tool-index list` | yes |
| Build `index/index.json` from the listing and the previous index; run `oyzu tools resolve` and `oyzu tools evidence` for new launch-set versions, and `oyzu tools evidence --provider <name>` for new core-provider versions | `tool-index build` | evidence downloads |
| Decide whether the change is routine or needs a person, and write the pull request body | `tool-index review` | no |
| Find the highest `index-v<serial>` tag, failing on a gap or an odd tag | `tool-index-release serial` | no |
| Stamp a release copy of the committed index with `serial` and `expires` | `tool-index-release stamp` | no |
| Sign the stamped copy and verify the signature (in `release.yml` only) | `cosign attest-blob`, `cosign verify-blob-attestation` | Sigstore |
| Check the release directory and print the release for tag `index-v<serial>` | `tool-index-release release` (`--dry-run` reports missing assets instead of failing) | no |
| Build, sign and verify the plugin catalog `plugins.json` beside the index, isolated from the tools release (in `release.yml` only; see [Plugin catalog](#plugin-catalog)) | `.github/scripts/plugin-catalog.sh` | Sigstore, GHCR (only when `config/plugins.toml` lists releases) |
| Publish (in `release.yml` only) | `gh release create` | GitHub |

### Format

`index.json` follows the oyzu client's contract,
[`tool-index.schema.json`](https://github.com/oyzuai/oyzu/blob/HEAD/docs/contracts/tools-v1/tool-index.schema.json):
one entry per tool, keyed by its canonical `oyzu:` id (the curated short name
in `config/launch-set.toml`, the same one the CLI writes into the lock,
otherwise `oyzu:<owner>/<repo>` lowercased), with its `origin`
(`aqua:<package>` or `core:<provider>`), serving class, and normalized versions
newest first. Each version carries its release date (`publisher` or
`first-seen`) and, at the verified tier, one artifact-set record per platform
exactly as `oyzu tools evidence` emitted it. A withdrawn version also
carries `"withdrawn": {"reason": "..."}` (a reason of 1 to 512 characters; see
[Withdrawals](#withdrawals)); the field is omitted otherwise, so an index
without withdrawals is byte-identical to one built before the field existed.
The job checks the client's structural rules before it writes the file.

The workflows in `.github/workflows` run these on GitHub-hosted runners.
`tool-index` builds against `oyzu-recipe` from private oyzu; the release steps
are a separate crate, `release-tool/` (binary `tool-index-release`), that
shares the index, stamp, release and serial modules by path and builds from
crates.io only. So the daily re-sign keeps working if the oyzu read App
breaks, and the index cannot expire for that reason.

### Tiers

- **Verified**: the 94-tool curated launch set in `config/launch-set.toml`.
  A record is verified by the publisher's checksum or by a GitHub artifact
  attestation (age, cargo-binstall and zizmor publish only attestations);
  `oyzu tools evidence` lists attestations from the GitHub API with the
  workflow's `GITHUB_TOKEN`. slsa-verifier publishes only a SLSA provenance
  file, which `oyzu` cannot verify yet, so it stays at the discovery tier.
  When a tool joins, its newest `backfill` releases are resolved and
  downloaded; afterwards every new release is, once. A run verifies at most
  `max_versions_per_run` versions: versions never tried first, then failed
  versions whose retry is due, each in core-provider then launch-set file
  order (not registry order, which starved tools late in the alphabet). A
  version counts as done once every platform is verified or `unsupported`; a
  version with any platform still failing is recorded in `index/attempts.json` with
  the failed check and waits `retry_after_days` (7) before the next try, or
  is retried at once when the job's oyzu pin changes. A core-provider
  version whose record says oyzu does not install it on one of
  `recheck_platforms` (`linux/arm64/gnu`, added to oyzu after Java, Node, Go
  and Python had records) is verified again once per oyzu pin;
  `index/attempts.json` remembers the check. Records are the exact
  `oyzu tools evidence` output, so their digests still verify. Records are
  kept for each tool's newest `keep_records` versions (10); older versions
  keep their dates, which bounds the single index file.
- **Discovery**: every other `github_release` and `http` package in the
  snapshot, with versions and dates only. A standalone client evaluates the
  recipe itself; managed mode only uses verified records.
- **Core providers**: Oyzu's own providers, built into the job rather than
  read from the registry, indexed as `oyzu:<name>` with origin `core:<name>`:
  Java (Eclipse Temurin, listed from Adoptium), Node (nodejs.org/dist), Go
  (go.dev/dl), Python (python-build-standalone's current release), Maven
  (archive.apache.org), Gradle (services.gradle.org), npm and pnpm (the npm
  registry), the same feeds the CLI installs from, and Helm, kubectl, k3d,
  Liquibase, Tailscale (stable track) and Rust, listed from the publisher's
  release tags because the CLI asks those publishers only for one exact
  version. Rust records carry the toolchain component tarballs the CLI
  installs, with the digests of the release's channel manifest. With
  `verified = true` in `config/index.toml` (set for all of them), new
  versions are verified like a
  launch-set tool (same `backfill` and per-run budget): `oyzu tools evidence
  --provider <name> --version <v> --platform <p>…` builds the records from
  the publisher's listing and checks each archive against the publisher's
  SHA-256 file. Records are produced for the platforms oyzu installs each
  provider on (Linux x86-64 and arm64, macOS arm64, Windows x86-64, where
  the publisher ships them); the other configured platforms get
  `unsupported` records. With `line = N`, versions, the backfill and the
  records kept count per release line (Node, Java, Gradle, npm and pnpm by
  major; Go, Python, Maven, Helm and kubectl by minor), so `node@lts` or `java@21` finds verified
  records even when newer lines release more often. Managed clients resolve
  these providers only from their records; standalone clients prefer them.

Versions a recipe does not cover (`version_filter`, `version_prefix`) are
dropped with the same engine the CLI uses (`oyzu-recipe`, pinned to the same
oyzu commit). A version must start with a number, or be a word, a dot and a
number (MinIO's `RELEASE.2025-08-13T08-35-41Z`); a date-shaped version's
hyphens do not make it a pre-release. Where a publisher gives no date, the index records the first run
that saw the version (decided 2026-10-10).

### Withdrawals

`config/withdrawn.toml` lists versions people withdrew from the index, one
`[[version]]` table each (the file ships with none, and a missing file
withdraws nothing):

```toml
[[version]]
tool = "oyzu:golang-migrate"   # canonical index id
version = "4.20.0"             # normalized, as the index lists it
reason = "upstream release assets were deleted (404)"
```

The job refuses the file when a `tool` is not an `oyzu:` id, a `reason` is
empty or over 512 characters, or a tool and version are listed twice. Each
matching version in `index.json` gains `withdrawn` with the reason and keeps
its records (`sets`), so a client can still match a digest to it and refuse it
by name; the CLI skips it when resolving a constraint and refuses an exact
request or a lock naming it (`TOOL_VERSION_WITHDRAWN`). While listed, a
withdrawn version the previous index had is never dropped (not when upstream
stops listing it, not by `discovery.max_versions`), is never verified again
(it takes no budget and its `index/attempts.json` entry is forgotten), and its
records never age out under `keep_records` (nor count toward it). Removing the
entry lifts the withdrawal; the version then ages out like any other. An
entry naming a tool or version the built index does not list does not fail
the build: the build report lists it under `withdrawals_not_indexed`, and
`tool-index review` (which reads `withdrawn.toml` from `--config`, default
`config`) notes it in the pull request body.

### Review rules

Routine pull requests merge automatically: new versions of indexed tools from
hosts already in the index. Everything else waits for a person: a new tool (a
renamed id counts as a removed and a new tool), a new download host, a new
registry snapshot or resolve engine, a changed origin or serving class, a
removed tool or verified version, a changed release date, a version withdrawn,
no longer withdrawn or with a changed withdrawal reason (each listed in the
pull request body), a withdrawn version leaving the index, and any change to a
published verified record. A retry keeps a platform's verified record as it
was and replaces only the others, whose new records count as added records, so
a retry that succeeds merges as routine unless it brings a new host. `serial` and `expires` are ignored, so a re-stamp
never needs review.

The `verify` workflow recomputes the decision on every pull request and every
review. Its `verify` check fails only on a real problem: a broken build or
test, or a pull request labeled `routine` that is not routine. The needs-review
gate, which rulesets cannot express, is the separate `index-review` commit
status: success when the decision needs no review or someone with write access
has approved the current head commit, pending ("Needs review") otherwise. A
valid `needs-review` update therefore shows `verify` green and `index-review`
pending until it is approved, and the approval's run turns the status green.
The build workflow opens and auto-merges pull requests as the
`oyzu-index-bot` GitHub App, because pull requests and merges made with
`GITHUB_TOKEN` start no workflows: neither `verify` nor `release.yml` would
run.

### Serving classes

`config/serving.toml` holds each launch-set tool's declared license and its
serving class. Every entry has been reviewed (`reviewed = true`, serving-class
review of 2026-10-10); an entry added later starts as a proposal
(`reviewed = false`) and stays `direct` until a person checks the license
shipped inside the archives and sets `reviewed = true`.

Temurin (`core:java`) is GPLv2 with the Classpath Exception: it is `cache` with
the matching source mirrored.

**Source mirroring.** A reviewed tool whose license is GPL, LGPL or AGPL and
whose class is `cache` is marked `source_mirror` in the index. For each of its
versions with records, `tool-index build` finds the source archive (GitHub's
tag archive for an aqua package, the `jdk-sources` package in the same Temurin GitHub
release for Java, or the `source` URL template in the tool's `serving.toml` entry),
downloads it once and records its URL, SHA-256 and size as the version's
`source`. A delivery service stores those bytes beside the binary and serves
a `source_mirror` version without a `source` directly from upstream. A changed
archive for a version that already had one needs review. Oracle JDK, Oracle GraalVM and the Android SDK are never proxied:
any tool id naming `oracle`, `graalvm` or `android` stays `direct` (or
`excluded`) whatever `serving.toml` says.

### Signing

Sigstore keyless signing (decided by Micah Martin, 2026-10-10; it replaced a
TUF design the same day). There is no key to hold: `release.yml` signs with a
short-lived certificate for its own GitHub OIDC identity, and the signature is
recorded in the public Rekor transparency log.

On every change to `main` and once a day, `release.yml` in the `release`
environment:

1. lists the `index-v*` tags and downloads the `index.json` of the highest
   one, not of the release GitHub marks latest (a person can move that).
   `tool-index-release serial` fails on a gap, a repeated serial or a malformed
   `index-v` tag, and a tag without its release fails the download;
2. runs `tool-index-release stamp --tags`, which checks the downloaded copy
   carries that highest serial and writes `release/index.json`: the committed
   `index/index.json` plus `serial` (published serial + 1, or 1) and `expires`
   (now + `signing.expires_days` in `config/index.toml`, 14). The committed
   copy never carries either field; `stamp` refuses one that does, and
   `review` fails on it;
3. writes an in-toto v1 statement (predicate type
   `https://sigstore.dev/cosign/sign/v1`, empty predicate) naming `index.json`'s
   SHA-256 and signs it with `cosign attest-blob --statement`, writing
   `index.json.sigstore.json`. `cosign sign-blob` would write a message
   signature and attest-blob's own statement is in-toto v0.1; the CLI accepts
   neither;
4. runs `cosign verify-blob-attestation` against the identity the CLI pins;
5. compresses `index.json` with `zstd -19` into `index.json.zst`;
6. runs `tool-index-release release --dir release` (which checks the compressed copy
   decompresses to exactly `index.json`) and `gh release create
   index-v<serial> --latest` with the four assets.

A release has four assets (six with the [plugin catalog](#plugin-catalog)) in one flat directory, the same names a mirror
such as `index.oyzu.dev` will serve later:

| Asset | What it is |
| --- | --- |
| `index.json` | The stamped index |
| `index.json.zst` | The same bytes, zstd-compressed; what the CLI fetches (it falls back to `index.json` on a 404). Transport only: the signature is over the decompressed bytes |
| `index.json.sigstore.json` | The Sigstore bundle (v0.3): a DSSE-signed in-toto statement naming `index.json`'s SHA-256, with the certificate and Rekor entry |
| `NOTICE` | The aqua registry's license text |

The CLI accepts a signature only from
`https://github.com/oyzuai/tool-index/.github/workflows/release.yml@refs/heads/main`,
issuer `https://token.actions.githubusercontent.com`, on a GitHub-hosted
runner, and rejects an expired copy or one with a lower `serial` than it has
seen. Renaming `release.yml`, signing from another branch or moving the
repository therefore breaks every client until a CLI release. The exact
checks are the oyzu client's contract.

## Plugin catalog

Each release also carries `plugins.json`, the signed catalog of Oyzu plugins
(draft contract `plugins-v1alpha1`, owned by oyzu:
[`catalog.schema.json`](https://github.com/oyzuai/oyzu/blob/HEAD/docs/contracts/plugins-v1alpha1/catalog.schema.json)),
and its bundle `plugins.json.sigstore.json`. Tools-v1, `index.json`, its
one-file transport and latest-release discovery are unchanged; a client that
knows only tools ignores the two extra assets. Tracking: tool-index#36,
oyzu#578.

**Location and source.** `config/plugins.toml` is the source of truth: the
catalog's `content` (authority, packages with releases, references,
detections, withdrawals) in TOML, in the order the contract requires.
`tests/fixtures/plugins-v1alpha1/plugins.go-preview.toml` is a full example.
It ships empty. The generator checks the whole file against the contract's
rules, so a mistake in an entry that is not advertised yet still fails review.

**Signing.** The `Plugin catalog` step of `release.yml` runs after the tools
assets are final, with the same keyless identity as `index.json`: an in-toto v1
statement (`https://sigstore.dev/cosign/sign/v1`) whose subject is
`plugins.json` and its SHA-256, signed with `cosign attest-blob`, then checked
with `cosign verify-blob-attestation` against the pinned identity. It is the
digest of the exact `plugins.json` bytes, not of any re-serialization.

**Freshness.** `serial` is the release's serial, equal to `index.json`'s
(a re-sign included). `generated_at` is the run's time; `expires_at` is the
index's expiry, cut to 31 days after `generated_at` (the client's cap). The
daily re-sign therefore moves `serial`, `generated_at` and `expires_at`, and
leaves `content_digest` (the SHA-256 of the RFC 8785 canonical JSON of
`content`) unchanged when the content is, so clients see an envelope refresh,
not a change.

**Catalog last.** Listing a release in `config/plugins.toml` does not
advertise it. `.github/scripts/plugin-availability.sh` keeps a release only
when its OCI release root is fetchable anonymously from GHCR by its digest and
the root has attached evidence (a Sigstore bundle among its OCI referrers, read
from the referrers API and, where that answers 404 as ghcr.io does, from the
OCI referrers tag schema index `manifests/sha256-<hex>`);
the tag index is writable by anyone who can push a tag, so an entry of the
evidence type counts only when the referrer manifest it names exists in the
repository, hashes to the entry's digest and has the root as its `subject`
(the registry vouches for that on the referrers API, so nothing more is
checked there). The `Link` rel=next pages of the referrers API are followed
(at most 50, only within the same route; a `rel` counts when its
space-separated tokens include `next`, in any case), and a response that is not
exactly one JSON object, or that holds a malformed entry, fails the run, as does
a transfer cut short. A tag index with more than 50 evidence entries fails the
run too. The script also rejects a root digest or repository that is not in OCI
form; the repository pattern (lowercase, `[a-z0-9._-]` segments) is deliberately
stricter than the OCI name rule. A release is left out only on a definite 404 (unknown manifest, no referrers
in either place) or
when no evidence is attached, and with no verification nothing is advertised.
Any other answer, such as a timeout, 5xx or 401/403, fails the run, so a GHCR
blip makes plugins absent from that release instead of publishing a catalog
that lacks packages (calls use `--max-time 30 --retry 3`). A package left without releases drops out together
with its references and detections. Withdrawals are always kept (they only
restrict). The evidence artifact type in that script is a placeholder until
oyzu S2c (#584) publishes real packages; the script's tests use a fake
registry, and the 404 and tag index lookups were also run anonymously against
an existing public ghcr.io image (not one of ours). Until then the source is empty and so is the published
list, which is still well formed.

**Failure isolation.** `plugin-catalog.sh` runs the plugin steps in their own
shell, under `timeout 600` (so a hung GHCR, Fulcio or Rekor call cannot run
into the job limit after `index.json` is signed), and always succeeds. If generation, signing or verification fails, any
partial `plugins.json` is removed, a workflow warning is raised, and the
release goes out with the four tools assets. `release` plans the plugin
assets separately: a catalog whose serial differs from `index.json`'s, that has
expired, fails the contract, or whose bundle does not name `plugins.json`'s
exact bytes is left out with the reason in the plan, and cannot fail it. The
publish step uploads exactly the assets in the plan. Plugins are then absent
from that release; clients keep their last verified catalog until it expires.

**Contract checks.** The release tool cannot depend on private oyzu, so it
carries a port of the semantic rules of `oyzu-plugin-protocol`'s catalog
(`src/plugins.rs`), synced to oyzu `8cde9a1` (after oyzu PR #610): the
`content_digest` over the content exactly as parsed, sorting and uniqueness,
references between members, version and major agreement, matcher bounds
(including no `not` directly inside `not`), the 31-day window, the 4 MiB limit,
one root digest per release of a package across releases and withdrawals, and
the route rule (publisher `oyzu`, and only it, is routed from
`ghcr.io/oyzuai/plugins/<name>`). There is no per-package `authority`; only
`content.authority`. Its tests run the vendored canonical examples (their
digests must match) and every catalog invalid and valid case from oyzu against
the port; `tests/plugin-catalog.sh` also validates generated output against the
canonical JSON Schema. Refresh `tests/fixtures/plugins-v1alpha1` from oyzu to
pick up contract changes; failing tests then show where the port differs. Not
ported: the client's strict decoding (it refuses duplicate JSON keys, an
explicit `null` for an optional field, and an array where an object belongs;
the generator writes the document itself and never produces these) and the client's serial succession (it needs the previous
catalog; the daily serial only grows).

**Retention.** The catalog lists supported, retained releases and keeps its
withdrawals. Pruning a release is done by removing it from `config/plugins.toml`
and means unlisted, not withdrawn: a locked selection still replays by its root
digest where policy permits, while new resolution no longer offers it. The
generator never drops a withdrawal, even when the release's bytes are
unavailable (it removes only an advisory `replacement` that is no longer
listed). Keep the document inside the 4 MiB limit by pruning, not by
withdrawing.

**Review rules.** Plugin entries have no routine path. `config/plugins.toml`
is code-owner reviewed like `.github/`, and the `verify` workflow's job
summary lists every difference from the base branch, capability-sensitive ones
first: detector reach (any detection, and added or changed references), new
requirements (roles and core features, protocol generations, platforms and
minimum platform), the source or signer (routes, root digests, publisher,
maintainer, and the catalog authority) and new or removed packages. Everything added or removed is marked,
withdrawals included, so un-revoking a release shows. Only qualification or preview-flag
edits are listed unmarked. Preview it locally with
`tool-index-release plugins review --previous <old> --next config/plugins.toml`.

## Run it locally

```sh
cargo build --release
T=./target/release/tool-index
$T fetch
$T list --source git --launch-set-only        # no token needed; no release dates
cargo build --manifest-path ../../oyzu/Cargo.toml --bin oyzu   # or any oyzu on PATH
$T build --listing work/listing.json --oyzu ../../oyzu/target/debug/oyzu --out index
$T review --next index/index.json
cargo build --release --manifest-path release-tool/Cargo.toml
R=./release-tool/target/release/tool-index-release
git ls-remote --tags origin 'refs/tags/index-v*' | sed -e 's#.*refs/tags/##' -e 's#\^{}$##' | sort -u > tags.txt
$R serial --tags tags.txt                    # then download that release's index.json into published/
$R stamp --published published/index.json --tags tags.txt --out release/index.json
cp index/NOTICE release/
$R release --dir release --dry-run           # lists index.json.sigstore.json as missing until cosign signs
$R plugins generate --index release/index.json --out release/plugins.json   # empty catalog, same serial
$R plugins check --file release/plugins.json --index release/index.json
tests/plugin-catalog.sh                      # the plugin workflow steps with a fake cosign and GHCR
```

Signing needs the workflow's OIDC identity, so it is not run locally; a
locally signed bundle would carry your identity, which no client accepts.

`cargo test` covers the job without the network: build and review on a
fixture registry, budget order and retry back-off, withdrawals (kept, never
re-verified, records kept, reviewed), Java records from a stand-in `oyzu tools evidence --provider`, stamping (serial, expiry, refusing a stamped input, following the highest tag), review
ignoring the stamp, and the release check (assets present, unexpired, and a
bundle naming `index.json`'s exact bytes), and the plugin catalog (the canonical examples and
invalid cases against the port, empty and non-empty generation, the content digest across
re-signs, catalog-last filtering, review of sensitive changes, and a catalog that cannot stop
the tools release). `tests/plugin-catalog.sh` runs the workflow's plugin steps end to end with
a fake cosign and a fake GHCR.

## Repository setup

These settings are part of the trust model; keep them when changing the
repository.

- The secrets `INDEX_BOT_APP_ID` and `INDEX_BOT_PRIVATE_KEY` are the
  `oyzu-index-bot` GitHub App, installed on this repository only (contents and
  pull requests, read and write). The bot must not be
  the maintainer who approves `needs-review` pull requests, since GitHub does
  not let an author approve their own pull request.
- The secrets `OYZU_READ_APP_ID` and `OYZU_READ_APP_PRIVATE_KEY` belong to a
  GitHub App installed on `oyzuai/oyzu` only, with Contents and Metadata
  read-only. oyzu is private, and `oyzu-recipe` and the job's `oyzu` binary
  are built from it. `.github/actions/oyzu-read` mints a one-hour token for
  that one repository before the cargo steps, and `done.sh` removes it right
  after, so no step that runs `oyzu` sees it. Fork pull requests get no
  secrets and cannot build. `release.yml` does not use it. Drop all of this
  once oyzu is public.
- The `release` environment is limited to `main`.
- A ruleset on `main` requires pull requests (no required approvals; the
  `index-review` status gates index changes), the `verify` check, the
  `index-review` status (source: GitHub Actions) and code-owner review, blocks
  force-push and deletion, and gives the bot no bypass.
  `CODEOWNERS` covers `.github/`, so the signing workflow cannot change
  without a maintainer.
- Auto-merge is allowed. Runners are GitHub-hosted only.

Later: serve the same release files from `index.oyzu.dev`.

## Dependency maintenance

The [Dependabot configuration](.github/dependabot.yml) maintains the root Cargo crate, the independent `release-tool` crate, workflows, and the local `oyzu-read` action. The two Cargo jobs are separate so a private-source access failure in the root job does not stop release-helper updates.

Routine version checks run on weekdays around 06:40 America/New_York. Minor and patch updates are grouped; major upgrades remain individual proposals. Security updates use a separate group and do not wait for the version-update schedule. Each update block allows three open routine PRs and assigns proposals to `micahlmartin`; GitHub applies a separate limit to security-update PRs. Review, applicable verification and existing merge requirements still apply.

Dependabot alerts and security updates must also be enabled in repository settings; the YAML does not enable those switches. Use the standard GitHub runner for update jobs. The configuration becomes active after it reaches the default branch.

The root crate depends on private `oyzuai/oyzu` source. Verify that Dependabot can read that dependency before treating the root job as operational; unavailable private-source access is a failed update job, not a clean result. The `oyzu-recipe` revision is intentionally ignored by automatic updates because it must be changed with the workflow's matching Oyzu binary pin.

Actions secrets are not automatically available to Dependabot-triggered checks. The existing private-source checkout needs approved Dependabot credential setup before those PR checks can build the root crate. Keep the read-only App and existing verification gates; do not run Dependabot changes with elevated `pull_request_target` privileges as a workaround. This configuration adds no credential, access grant, auto-merge rule, or signing change.
