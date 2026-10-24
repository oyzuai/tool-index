# Vendored plugins-v1alpha1 contract files

Copied unchanged from `oyzuai/oyzu` at commit
`8cde9a1bc661e66f9965a6ce3b766611977017e2`
(`docs/contracts/plugins-v1alpha1/`). The canonical contract lives there; these
copies let the release tool's tests run without access to private oyzu. To
refresh, copy the same files from a newer oyzu commit and update this SHA; the
tests then show whether the port in `src/plugins.rs` still agrees.

Issue #62 extends this existing public port in place for the concurrently
reviewed local Preview `root_preimage` contract: the optional catalog-schema
property and three proof-field mutations are authored here against that public
wire requirement. The older base examples and all other schema definitions
retain the revision above. `e30=` is a wire-shape fixture only, not an approved
immutable local-Preview root. No private Oyzu Rust implementation is vendored.
