# Maintainer-local Preview catalog publication

Tracking: [issue #62](https://github.com/oyzuai/tool-index/issues/62).

Status: draft implementation plan. No catalog, signature or release has been
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
