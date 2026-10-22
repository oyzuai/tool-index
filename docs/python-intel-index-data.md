# Intel macOS Python index preparation

Tracking: [tool-index#52](https://github.com/oyzuai/tool-index/issues/52),
[engine pin #51](https://github.com/oyzuai/tool-index/pull/51), and
[oyzu#884](https://github.com/oyzuai/oyzu/issues/884).

This draft prepares unsigned index data for the Intel macOS Python client
change merged in [oyzu#885](https://github.com/oyzuai/oyzu/pull/885).
It does not publish a signed index or qualify a native interpreter installation.
The engine pin is `b394857b632be050cd69beedb868a17bf0e27ea9`;
collector output must come from that exact source revision.

## Selective build constraint

The current `tool-index build --only core:python` command emits only selected
tools. In `src/build.rs`, the output tools vector starts empty, and both the
core-provider loop and registry loop skip every unselected origin. The previous
index supplies versions and records only for tools that reach those loops.
There is no carry-forward of unrelated tools into the output vector.

Consequently, the selective command cannot replace the global index safely.
Do not splice its records into the global index manually. A complete normal
builder run, or an explicitly reviewed builder change, is needed before this
draft can contain replacement index data. A complete run also retries failed
versions and rechecks unsupported core records under the new engine revision;
that broader activity needs an explicit scope decision for this preparation.

## Review requirements

Before data is ready, retain the real collector output and builder report,
run the normal index review against the previous committed index, and compare
all unrelated tools, release dates, serving facts, and previously verified
records. Record exact versions, platforms, counts, digests, engine identity,
and real failures. Keep `serial` and `expires` absent; release stamping and
signing remain owned by the ordinary release workflow.

Current status: draft checkpoint only; no generated index data yet.
