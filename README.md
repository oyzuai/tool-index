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
| List upstream versions: GitHub GraphQL in production (`git ls-remote` for owners whose IP allow list refuses the token), `git ls-remote` without a token; Adoptium, nodejs.org, go.dev and python-build-standalone for the core providers | `tool-index list` | yes |
| Build `index/index.json` from the listing and the previous index; run `oyzu tools resolve` and `oyzu tools evidence` for new launch-set versions, and `oyzu tools evidence --provider <name>` for new core-provider versions | `tool-index build` | evidence downloads |
| Decide whether the change is routine or needs a person, and write the pull request body | `tool-index review` | no |
| Find the highest `index-v<serial>` tag, failing on a gap or an odd tag | `tool-index-release serial` | no |
| Stamp a release copy of the committed index with `serial` and `expires` | `tool-index-release stamp` | no |
| Sign the stamped copy and verify the signature (in `release.yml` only) | `cosign attest-blob`, `cosign verify-blob-attestation` | Sigstore |
| Check the release directory and print the release for tag `index-v<serial>` | `tool-index-release release` (`--dry-run` reports missing assets instead of failing) | no |
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
  is retried at once when the job's oyzu pin changes. Records are the exact
  `oyzu tools evidence` output, so their digests still verify. Records are
  kept for each tool's newest `keep_records` versions (10); older versions
  keep their dates, which bounds the single index file.
- **Discovery**: every other `github_release` and `http` package in the
  snapshot, with versions and dates only. A standalone client evaluates the
  recipe itself; managed mode only uses verified records.
- **Core providers**: Oyzu's own providers, built into the job rather than
  read from the registry, indexed as `oyzu:<name>` with origin `core:<name>`:
  Java (Eclipse Temurin, listed from Adoptium), Node (nodejs.org/dist), Go
  (go.dev/dl) and Python (python-build-standalone's current release), the
  same feeds the CLI installs from. With `verified = true` in
  `config/index.toml` (set for all four), new versions are verified like a
  launch-set tool (same `backfill` and per-run budget): `oyzu tools evidence
  --provider <name> --version <v> --platform <p>…` builds the records from
  the publisher's listing and checks each archive against the publisher's
  SHA-256 file. Records are produced for the platforms oyzu installs these
  providers on (Linux x86-64, macOS arm64, Windows x86-64); the other
  configured platforms get `unsupported` records. With `line = N`, versions,
  the backfill and the records kept count per release line (Node and Java by
  major, Go and Python by minor), so `node@lts` or `java@21` finds verified
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

`config/serving.toml` holds each launch-set tool's declared license as a
**proposal** (`reviewed = false`). An unreviewed tool stays `direct`; the index
shows the proposed license. A person sets `reviewed = true` after checking the
license shipped inside the archives.

Temurin (`core:java`) is GPLv2 with the Classpath Exception: once reviewed it
is `cache` with the matching source mirrored; until then `oyzu:java` is
`direct`.

**Source mirroring.** A reviewed tool whose license is GPL, LGPL or AGPL and
whose class is `cache` is marked `source_mirror` in the index. For each of its
versions with records, `tool-index build` finds the source archive (GitHub's
tag archive for an aqua package, the Adoptium release's source package for
Java, or the `source` URL template in the tool's `serving.toml` entry),
downloads it once and records its URL, SHA-256 and size as the version's
`source`. A delivery service stores those bytes beside the binary and serves
a `source_mirror` version without a `source` directly from upstream. A changed
archive for a version that already had one needs review. Nothing is mirrored
until a copyleft tool is reviewed. Oracle JDK, Oracle GraalVM and the Android SDK are never proxied:
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

A release has four assets in one flat directory, the same names a mirror
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
```

Signing needs the workflow's OIDC identity, so it is not run locally; a
locally signed bundle would carry your identity, which no client accepts.

`cargo test` covers the job without the network: build and review on a
fixture registry, budget order and retry back-off, withdrawals (kept, never
re-verified, records kept, reviewed), Java records from a stand-in `oyzu tools evidence --provider`, stamping (serial, expiry, refusing a stamped input, following the highest tag), review
ignoring the stamp, and the release check (assets present, unexpired, and a
bundle naming `index.json`'s exact bytes).

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

Not yet done: review of the serving-class proposals in `config/serving.toml`.
Later: serve the same release files from `index.oyzu.dev`.
