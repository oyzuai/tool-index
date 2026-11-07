# Session context

`settings.json` runs `load-context.sh` at SessionStart with a 60-second timeout.
It clones or fast-forwards the sibling `context` checkout on `main`, then prints
the committed `CONSTITUTION.md`, `INDEX.md` and `context commit <sha>`. It also
refreshes the sibling `agent-workflows` source and links shared/repository skills
without replacing existing packages. Bash, Git and authenticated private access
to `oyzuai/context` are required. Existing hooks and permissions are preserved.

A failed context load prints `ERROR: ... context load FAILED` to both stderr
and session stdout; a skill refresh failure also prints an `ERROR`. The hook
returns zero so offline repository work remains possible. The explicit error
marker means shared context did **not** load; restore access or correct the cache
branch/origin and retry before relying on shared policy. A host-enforced timeout
may instead be reported by Claude. Skill collisions retain their existing warning.

Run `bash .claude/test-load-context.sh` from the repository root. The plain Bash
test uses temporary local bare remotes, isolates Git configuration, forbids all
network protocols, and checks initial load, refresh, unavailable cached/fresh
remotes and missing documents. It removes its own temporary files on exit.

Repository hooks apply to repository sessions. For a cloud session starting
outside a repository, install the separately provisioned user-level hook as
described in [agent-workflows adoption](https://github.com/oyzuai/agent-workflows/blob/main/adoption/README.md#cloud-session-context).
These repository files do not install cloud user settings.
