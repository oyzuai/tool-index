#!/usr/bin/env bash
# Builds, signs and verifies release/plugins.json beside the stamped
# release/index.json. Run by plugin-catalog.sh, which keeps any failure here
# from touching the tools release: plugins.json is then simply absent.
#
# The signing identity is the workflow's own (the same one that signed
# index.json); the statement names plugins.json's exact bytes.
set -euo pipefail

dir=${RELEASE_DIR:-release}
tool=${RELEASE_TOOL:-./release-tool/target/release/tool-index-release}
source=${PLUGIN_SOURCE:-config/plugins.toml}
availability=${PLUGIN_AVAILABILITY:-"$(dirname "$0")/plugin-availability.sh"}
cosign=${COSIGN:-cosign}

rm -f "$dir/plugins.json" "$dir/plugins.json.sigstore.json"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Catalog-last: only releases whose OCI root and evidence are available are
# advertised. With nothing in the source there is nothing to verify and the
# catalog is empty but well formed.
"$tool" plugins candidates --source "$source" > "$work/candidates.json"
verified=()
if [ "$(jq length "$work/candidates.json")" -gt 0 ]; then
  "$availability" "$work/candidates.json" > "$work/verified.json"
  verified=(--verified "$work/verified.json")
fi

"$tool" plugins generate --source "$source" --index "$dir/index.json" \
  "${verified[@]}" --out "$work/plugins.json"
"$tool" plugins check --file "$work/plugins.json" --index "$dir/index.json"

cp "$work/plugins.json" "$dir/plugins.json"
"$tool" plugins statement --file "$dir/plugins.json" > "$work/statement.json"
(cd "$dir" && "$cosign" attest-blob --yes --statement "$work/statement.json" \
  --bundle plugins.json.sigstore.json plugins.json)

(cd "$dir" && "$cosign" verify-blob-attestation --bundle plugins.json.sigstore.json \
  --type https://sigstore.dev/cosign/sign/v1 \
  --certificate-identity https://github.com/oyzuai/tool-index/.github/workflows/release.yml@refs/heads/main \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  plugins.json)
