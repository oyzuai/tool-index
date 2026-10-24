# Maintainer-local Preview catalog publication

Tracking: [issue #62](https://github.com/oyzuai/tool-index/issues/62).

Status: source implementation under review; checks are unrun. No catalog, signature or release has been
produced by this change. The maintainer authorized local Preview publication
and a separate exact human Preview signer; its account and certificate identity
are not yet bound. Actions stays disabled.

## Outcome and ownership

The independent `release-tool` helper will prepare the local Preview plugin
catalog without producing a replacement tool index or changing the existing
workflow-signed catalog. Reuse `src/plugins.rs` for catalog generation and
validation. Keep the root job's private recipe dependency, engine pin, generated
tool records and primary release workflow outside this change.

The public metadata destination is the fixed `plugins-preview` release, marked
prerelease and excluded from Latest. It contains only `plugins-preview.json`
and `plugins-preview.json.sigstore.json`. A separate immutable Preview CLI/shim
tag is also excluded from Latest. Existing `index-vN` releases and their assets
retain their current contracts and signing identities.

The client uses this fixed metadata base only for its exact default official
index settings. Configured official mirrors retain their configured base;
organization catalogs remain exclusive. The client owns signature admission,
freshness, rollback, selection, installation and replay. A publisher plan or
availability probe does not establish those guarantees.

## Required implementation

- Add a finite local Preview operation to the existing plugin command owner.
  Reuse catalog generation with an independent serial, current generation time
  and bounded expiry, without synthesizing a tool index. Initial serial is one;
  later publication follows the last independently verified Preview catalog.
- Produce a Statement/v1 subject naming `plugins-preview.json` and the digest
  of its exact bytes. Preserve the primary statement subject and behavior.
- Extend the existing catalog port in place for the canonical optional
  `root_preimage` withdrawal field. Preserve it in the content digest, enforce
  complete base64 syntax and the existing size bounds, and validate exact raw
  root hash, immutable local Preview marker, official package and version.
- Keep a separately reviewed Preview content source under maintainer ownership.
  It may list only official Preview releases and exact proved withdrawals.
  Never infer qualified platforms, roots, material or signer identity.
- Prepare a publication plan containing exactly the two metadata assets and
  the fixed non-Latest destination. Keep existing primary release planning and
  workflow verification unchanged.

Production Sigstore roots remain unchanged. The human certificate must match
the named approved provider/account with exact SAN type/value and issuer before
signing or client activation. Publisher login grants no consumer login
requirement. No agent supplies a human or legal approval.

## Smallest proof and completion

First prove local generation from a small genuine fixture, exact-byte statement
construction, expected asset planning, and refusal of invalid Preview authority
or withdrawal proofs through the actual compiled release helper. Run focused
owner tests and canonical contract fixtures. Preserve primary generation,
statement and release-plan results with regression coverage. Run all compiler
commands through the provisioned managed Oyzu toolchain; do not install another
compiler or start Actions.

Public delivery follows actual package publication: every listed root, required
notice/recipient material, payload and signature must already be anonymously
available. Local generation or registry reachability alone is insufficient.
Then verify the human-signed exact catalog pair and actual compiled-client
anonymous first use, usable project artifact/reports, second-project shared-store
reuse and exact frozen/offline replay. Record commands, source/artifact identities
and limits before calling that flow complete. No public package availability or
application proof has run at this draft checkpoint.

## Authored helper and operator handoff

The independent crate's `plugins preview` commands use the existing catalog
port and catalog-last generator, with no tool-index file or private recipe
dependency. The primary `plugins generate/check/statement`, `src/release.rs`
and `.github/scripts/plugin-catalog*` are unchanged. No dependency or lock change
is part of this source batch. The human account, SAN type/value and issuer remain
unbound; do not substitute a workflow identity or a test authority.

The Preview source starts empty. Before adding any of the eleven packages,
record the genuine binary version, immutable marked root, actual qualified
platforms, public payloads/referrers, required notices and recipient/source
material. Verify the exact root's normal human signature separately. The
existing availability record has `{releases:[{package,version,root_digest}]}`;
it is an operator input, not a signature or legal approval. The existing
availability script establishes limited anonymous root/referrer presence only;
it cannot replace root signature/material/functional checks. Missing verification
omits a release and its references/detections rather than inventing availability.

For initial publication, omit `--published` only after confirming the fixed
metadata release has never been published. Later, download its exact catalog
and bundle from `https://github.com/oyzuai/tool-index/releases/download/plugins-preview/`
and independently verify the pair with the named approved identity **before**
passing those exact bytes as `--published`. Retain that verification receipt and
its catalog digest. The helper cannot determine remote history or perform
cryptographic verification. An expired prior catalog still establishes the next
serial after valid independent signature verification; it cannot authorize
client acquisition. Generation rejects a clock preceding the prior generation
and always uses the previous serial plus one.

After candidate/helper qualification and identity binding, the ordinary publisher
operations have this shape; these are templates, not commands already run:

```sh
tool-index-release plugins preview generate --source config/plugins-preview.toml \
  --published "$WORK/prior/plugins-preview.json" --verified "$WORK/verified.json" \
  --out "$DIR/plugins-preview.json"
tool-index-release plugins preview check --file "$DIR/plugins-preview.json"
tool-index-release plugins preview statement --file "$DIR/plugins-preview.json" \
  > "$WORK/preview-statement.json"
cosign attest-blob --yes --statement "$WORK/preview-statement.json" \
  --bundle "$DIR/plugins-preview.json.sigstore.json" "$DIR/plugins-preview.json"
cosign verify-blob-attestation --bundle "$DIR/plugins-preview.json.sigstore.json" \
  --type https://sigstore.dev/cosign/sign/v1 \
  --certificate-identity "$APPROVED_EXACT_SAN" \
  --certificate-oidc-issuer "$APPROVED_EXACT_ISSUER" "$DIR/plugins-preview.json"
tool-index-release plugins preview release --dir "$DIR"
```

The identity variables must contain the approved, independently verified exact
certificate values; they currently have no values. For the first release the
coordinator's reviewed upload selects only the pair:

```sh
gh release create plugins-preview --repo oyzuai/tool-index --prerelease --latest=false \
  --title 'Local Preview plugin catalog' --notes-file "$WORK/reviewed-public-notes.md" \
  "$DIR/plugins-preview.json" "$DIR/plugins-preview.json.sigstore.json"
```

For an existing fixed metadata release, inspect its tag, prerelease/non-Latest
state and current pair first; the coordinator replaces only those two reviewed
assets with `gh release upload plugins-preview --repo oyzuai/tool-index --clobber`
and the two exact paths. GitHub asset replacement is not atomic; preserve prior
verified bytes/receipts and verify the anonymously downloaded final pair after
upload. Never substitute this metadata tag for the separate immutable CLI tag,
mark either Latest, upload unrelated files or modify the primary release.
These upload steps require the coordinator's actual qualified artifact/material
acceptance and named identity, not merely a successful helper plan.

Preview withdrawal proofs are raw bytes: the port retains `root_preimage` in
the signed content digest and checks complete canonical base64/bounds. The
Preview owner additionally checks exact SHA-256, OCI release-root shape,
`local-preview` annotation and official package/version annotations. It does
not fetch payloads or claim a full graph verification from that proof. Old
ordinary catalogs can omit the field and retain their prior digests.

## Pending bounded verification

No build, format, tests, cosign operation, network availability check or upload
has run for this source batch. The smallest admitted sequence is managed offline
format/check and `cargo test --locked --offline --manifest-path release-tool/Cargo.toml`,
then the compiled helper's tiny generate → check → statement → signed-pair plan
flow. Its new component fixtures deliberately have synthetic unsigned bundles;
they prove byte/shape planning only. The existing canonical catalog mutation
tests remain included through the same port, and the existing primary release
tests retain their assertions. Independent root-crate release tests need an
already accepted compatible private recipe dependency and separate admission;
do not fetch private source or treat their omission as a pass.

Applicable docs/schema checks use the already provisioned validator environment;
no installation is authorized by this plan. Windows/Linux/macOS helper behavior,
real human certificate acceptance and anonymous client installation/reuse remain
unqualified until measured. The existing human authorization for local Preview
publication remains in force; this plan does not waive the remaining artifact,
identity, material or legal gates or confer an agent's approval.
