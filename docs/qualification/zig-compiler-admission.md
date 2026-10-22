# Managed Zig compiler admission

Status: source implementation prepared; managed source checks pass. Generated
admission and signed publication have not run for this source change.

This is the tool-index source prerequisite for
[Zig dependency preparation](https://github.com/oyzuai/oyzu/issues/875), tracked
in [issue #57](https://github.com/oyzuai/tool-index/issues/57). Accepted client
source [oyzu#895](https://github.com/oyzuai/oyzu/pull/895) admits Intel macOS and
Linux amd64 GNU. That does not itself create a signed tool-index record.

## Source ownership and selection

The engine pin in `Cargo.toml` and the two matching `Cargo.lock` Git nodes adopt
accepted client revision `79fe5de03a1543f79aaeac69f6d47fda24b10c6e`.
The ordinary workflow derives its recipe engine and separately built evidence
CLI from that same pin. Targeted `cargo update -p oyzu-recipe --precise` changed
only the `oyzu-recipe` and `oyzu-yaml` source identities; no registry dependency
was changed. The recipe and YAML source trees are unchanged from the earlier
`b394857b632be050cd69beedb868a17bf0e27ea9` pin.

`src/versions.rs` lists canonical stable `MAJOR.MINOR.PATCH` keys from the
[official download catalog](https://ziglang.org/download/index.json), excludes
`master`, development/pre-release/build suffixes and noncanonical numeric keys,
sorts numerically newest first, and retains valid publisher release days.
`src/main.rs` routes `core:zig` to that lister. Platform archives, layouts and
checksums remain owned by the existing `oyzu tools evidence --provider zig`
collector and the accepted client metadata implementation.

`config/index.toml` adds verified `core.zig`, canonical `oyzu:zig`, using the
existing minor-line policy (`line = 2`, `lines = 3`, `keep_records = 3`,
`max_versions = 30`). The first build backfills up to three versions per
retained line, at most nine versions and eighteen supported-platform archive
checks, within the existing global budget. Other configured platforms retain
explicit unsupported records. Future new minor lines rotate this window under
the same ordinary index retention and review rules.

The required compiler remains the agreed **0.15.2**. The pinned ZLS 0.15.0
consumer's minimum is 0.15.1; historical 0.14 archive-layout fixtures do not
change that baseline. On 2026-10-22, the official catalog's newest stable lines
were 0.17, 0.16 and 0.15, with four versions in this window: 0.17.0, 0.16.0,
0.15.2 and 0.15.1. Thus the normal per-line backfill reaches 0.15.2 without a
special version override. The metadata-only observation used:

```sh
curl -fsSL https://ziglang.org/download/index.json -o zig-download-index.json
shasum -a 256 zig-download-index.json
```

The observed catalog was 83,445 bytes, SHA-256
`8ae05dc5a2af0d1cd4c3df937eea4f2b94e3dfcbdfe013fc2b621612fb344e0f`.
It was fetched as data; no compiler archive was downloaded or installed.

## Existing index and review boundaries

Inspection of the base `d6f0442834ec0b62fa885655e462dbac3151b8ea` found only
`oyzu:ziglang/zig`, origin `aqua:ziglang/zig`, at the discovery tier with all
version `sets` empty. There was no `oyzu:zig` or `core:zig`. Preserve that
registry entry and its dates while adding the distinct native entry; no
canonical-ID migration or promotion of registry records is involved.

There are no settled native Zig records to revisit, so `recheck_platforms` is
unchanged. In particular, this change does not add a blanket Linux amd64
recheck across unrelated core providers. The next generated-data update adds
a new tool and its publisher host and must retain the normal human review gate.

Offline regression tests cover stable filtering/order/date handling and a
complete fixture build that uses production Zig configuration, backfills the
selected compiler's minor line, preserves unsupported targets, preserves the
registry Zig entry and unrelated verified records, and requires review for
the new native tool. Fixture evidence is a stand-in process, not real compiler
verification or signed admission.

Required evidence is bounded managed source verification, the exact changed
configuration and independent contract review. Preserve the complete global
index, existing records, review gates and signing workflow. Do not handwrite
admission, locks, receipts or attempts.

Normal generated-data review and signed publication remain separate authorized
operations. This source task installs no compiler, dispatches no workflow and
claims no Linux runtime or captured build proof.

Always use the complete normal build to prepare a replacement global index.
`list --only core:zig` is a metadata diagnostic; `build --only core:zig` emits a
filtered index and must not replace the global catalog. Do not hand-splice
records, receipts, attempts or locks. Normal generation, independent review,
signed publication, managed Linux installation and frozen replay remain
separate evidence gates.

## Managed source verification, 2026-10-22

Checks ran from the existing Oyzu-managed Rust project
`/Users/micahlmartin/Developer/oyzu-worktrees/zig-dependency-closure`, with
`cargo 1.99.1 (5f94df478 2026-08-27)`. The project environment remained
untrusted and skipped (`PROJECT_UNTRUSTED`); it was not enabled for these
commands. Each command used one build job, a physical task temporary directory,
and the existing Linux-admission target cache:

```sh
export CARGO_BUILD_JOBS=1
export CARGO_TARGET_DIR=/Users/micahlmartin/Developer/oyzu-proofs/issue-875/linux-admission/target
export TMPDIR=/Users/micahlmartin/Developer/oyzu-proofs/issue-875/index-admission/tmp
index_manifest=/Users/micahlmartin/Developer/tool-index-worktrees/zig-linux-index/Cargo.toml

CARGO_NET_GIT_FETCH_WITH_CLI=true /Users/micahlmartin/.local/bin/oyzu exec -- cargo update --manifest-path "$index_manifest" -p oyzu-recipe --precise 79fe5de03a1543f79aaeac69f6d47fda24b10c6e
/Users/micahlmartin/.local/bin/oyzu exec -- cargo metadata --manifest-path "$index_manifest" --locked --no-deps --format-version 1
/Users/micahlmartin/.local/bin/oyzu exec -- cargo fmt --manifest-path "$index_manifest" --all -- --check
/Users/micahlmartin/.local/bin/oyzu exec -- cargo test --manifest-path "$index_manifest" --locked zig_
/Users/micahlmartin/.local/bin/oyzu exec -- cargo test --manifest-path "$index_manifest" --locked --lib --test build
/Users/micahlmartin/.local/bin/oyzu exec -- cargo clippy --manifest-path "$index_manifest" --locked --all-targets -- -D warnings
```

| Check | Observed result |
| --- | --- |
| Targeted engine update | Exit 0; only the two Oyzu Git source nodes changed |
| Locked manifest metadata | Exit 0 |
| Formatting | Exit 0 |
| Focused Zig regressions | Both passed; initial compilation took 3m 07s |
| Library and build suite | 42 library tests and 18 build tests passed |
| Strict root Clippy, all targets | Exit 0, 1m 08s; no warnings |
| `git diff --check` | Exit 0 |

The baseline gap was established by index/config/source inspection; no
pre-change failing build was run. Independent first review of source checkpoint
`d3ffad2` found no issues. The separately packaged release helper was unchanged
and was not rebuilt; hosted verification remains owned by the normal PR flow.

Local logs and the fetched publisher metadata are retained under
`/Users/micahlmartin/Developer/oyzu-proofs/issue-875/index-admission`.
The reused target grew from about 2.8 GB to 3.8 GB and remains owned by the
active Zig qualification work. All source-verification compiler jobs stopped
before transferring the single heavy compiler slot to that work; no target,
shared cache, installed tool, or worktree was removed.
