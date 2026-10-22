# Managed Zig compiler admission

Status: source implementation and generated admission unrun.

This is the tool-index source prerequisite for
[Zig dependency preparation](https://github.com/oyzuai/oyzu/issues/875), tracked
in [issue #57](https://github.com/oyzuai/tool-index/issues/57). Accepted client
source [oyzu#895](https://github.com/oyzuai/oyzu/pull/895) admits Intel macOS and
Linux amd64 GNU. That does not itself create a signed tool-index record.

Use the existing publisher listing, client evidence and index assembly owners.
Adopt accepted client revision `79fe5de03a1543f79aaeac69f6d47fda24b10c6e`, add
stable Zig admission and preserve explicit unsupported targets. Inspect current
Zig index origin/records before changing settled-record recheck policy.

Required evidence is bounded managed source verification, the exact changed
configuration and independent contract review. Preserve the complete global
index, existing records, review gates and signing workflow. Do not handwrite
admission, locks, receipts or attempts.

Normal generated-data review and signed publication remain separate authorized
operations. This source task installs no compiler, dispatches no workflow and
claims no Linux runtime or captured build proof. Record actual commands,
observations, failures and remaining gates here as implementation proceeds.
