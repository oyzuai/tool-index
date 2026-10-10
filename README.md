# Tool index job

Builds, reviews and releases Oyzu's public tool index: the signed
metadata (versions, release dates, download pointers, SHA-256 digests and
serving classes) that the oyzu CLI reads instead of upstream feeds.

Status: **pre-launch.** Nothing has been signed or published yet; the first
release will be `index-v1`.

## What a run does

| Step | Command | Network |
| --- | --- | --- |
| Fetch the pinned aqua registry snapshot, its license (the `NOTICE`) and the mise name catalog, each checked against its pinned digest | `tool-index fetch` | yes |
| List upstream versions: GitHub GraphQL in production, `git ls-remote` without a token, Adoptium for Java | `tool-index list` | yes |
| Build `index/index.json` from the listing and the previous index; run `oyzu tools resolve` and `oyzu tools evidence` for new launch-set versions, and `oyzu tools evidence --provider java` for new Java versions | `tool-index build` | evidence downloads |
| Decide whether the change is routine or needs a person, and write the pull request body | `tool-index review` | no |
| Stamp a release copy of the committed index with `serial` and `expires` | `tool-index stamp` | no |
| Sign the stamped copy and verify the signature (in `release.yml` only) | `cosign sign-blob`, `cosign verify-blob` | Sigstore |
| Check the release directory and print the release for tag `index-v<serial>` | `tool-index release` (`--dry-run` reports missing assets instead of failing) | no |
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
exactly as `oyzu tools evidence` emitted it. The job checks the client's
structural rules before it writes the file.

The workflows in `.github/workflows` run these on GitHub-hosted runners.

### Tiers

- **Verified**: the 94-tool curated launch set in `config/launch-set.toml`.
  A record is verified by the publisher's checksum or by a GitHub artifact
  attestation (age, cargo-binstall and zizmor publish only attestations);
  `oyzu tools evidence` lists attestations from the GitHub API with the
  workflow's `GITHUB_TOKEN`. slsa-verifier publishes only a SLSA provenance
  file, which `oyzu` cannot verify yet, so it stays at the discovery tier.
  When a tool joins, its newest `backfill` releases are resolved and
  downloaded; afterwards every new release is, once. Records are the exact
  `oyzu tools evidence` output, so their digests still verify. Records are
  kept for each tool's newest `keep_records` versions (10); older versions
  keep their dates, which bounds the single index file.
- **Discovery**: every other `github_release` and `http` package in the
  snapshot, with versions and dates only. A standalone client evaluates the
  recipe itself; managed mode only uses verified records.
- **Core providers**: Java (Eclipse Temurin) is listed from Adoptium and
  indexed as `oyzu:java` with origin `core:java`. With `verified = true` in
  `config/index.toml` (set for Java), new versions are verified like a
  launch-set tool (same `backfill` and per-run budget): `oyzu tools evidence
  --provider java --version <v> --platform <p>…` builds the records from
  Adoptium's GA listing and checks each archive against the publisher's
  SHA-256 file. Records are produced for the platforms oyzu installs Temurin
  on (Linux x86-64, macOS arm64, Windows x86-64); the other configured
  platforms get `unsupported` records. Managed clients resolve `oyzu:java`
  only from these records.

Versions a recipe does not cover (`version_filter`, `version_prefix`) are
dropped with the same engine the CLI uses (`oyzu-recipe`, pinned to the same
oyzu commit). Where a publisher gives no date, the index records the first run
that saw the version (decided 2026-10-10).

### Review rules

Routine pull requests merge automatically: new versions of indexed tools from
hosts already in the index. Everything else waits for a person: a new tool (a
renamed id counts as a removed and a new tool), a new download host, a new
registry snapshot or resolve engine, a changed origin or serving class, a
removed tool or verified version, a changed release date, and any change to a
record already published. `serial` and `expires` are ignored, so a re-stamp
never needs review.

The `verify` workflow recomputes the decision on every pull request and every
review, and is the gate rulesets cannot express: a pull request labeled
`routine` that is not routine fails it, and a pull request whose decision is
`review` passes only once someone with write access has approved its current
head commit. The build workflow opens and auto-merges pull requests as the
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
`direct`. Oracle JDK, Oracle GraalVM and the Android SDK are never proxied:
any tool id naming `oracle`, `graalvm` or `android` stays `direct` (or
`excluded`) whatever `serving.toml` says.

### Signing

Sigstore keyless signing (decided by Micah Martin, 2026-10-10; it replaced a
TUF design the same day). There is no key to hold: `release.yml` signs with a
short-lived certificate for its own GitHub OIDC identity, and the signature is
recorded in the public Rekor transparency log.

On every change to `main` and once a day, `release.yml` in the `release`
environment:

1. downloads the latest release's `index.json`, if there is one, for its
   `serial`;
2. runs `tool-index stamp`, which writes `release/index.json`: the committed
   `index/index.json` plus `serial` (published serial + 1, or 1) and `expires`
   (now + `signing.expires_days` in `config/index.toml`, 14). The committed
   copy never carries either field; `stamp` refuses one that does, and
   `review` fails on it;
3. runs `cosign sign-blob --yes --bundle index.json.sigstore.json index.json`
   (cosign v3, pinned through `sigstore/cosign-installer` by commit);
4. runs `cosign verify-blob` against the identity the CLI pins;
5. compresses `index.json` with `zstd -19` into `index.json.zst`;
6. runs `tool-index release --dir release` (which checks the compressed copy
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
$T stamp --out release/index.json            # add --published <latest index.json> after the first release
cp index/NOTICE release/
$T release --dir release --dry-run           # lists index.json.sigstore.json as missing until cosign signs
```

Signing needs the workflow's OIDC identity, so it is not run locally; a
locally signed bundle would carry your identity, which no client accepts.

`cargo test` covers the job without the network: build and review on a
fixture registry, Java records from a stand-in `oyzu tools evidence --provider`, stamping (serial, expiry, refusing a stamped input), review
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
  secrets and cannot build. Drop all of this once oyzu is public.
- The `release` environment is limited to `main`.
- A ruleset on `main` requires pull requests (no required approvals; the
  `verify` check gates index changes), the `verify` check and code-owner
  review, blocks force-push and deletion, and gives the bot no bypass.
  `CODEOWNERS` covers `.github/`, so the signing workflow cannot change
  without a maintainer.
- Auto-merge is allowed. Runners are GitHub-hosted only.

Not yet done: review of the serving-class proposals in `config/serving.toml`.
Later: serve the same release files from `index.oyzu.dev`.
