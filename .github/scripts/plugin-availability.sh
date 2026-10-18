#!/usr/bin/env bash
# Usage: plugin-availability.sh candidates.json > verified.json
#
# Catalog-last. Prints {"releases": [...]} holding each candidate release
# (from `tool-index-release plugins candidates`) whose OCI release root is
# fetchable anonymously by digest and whose evidence is attached to it.
# Anything else is left out, with the reason on stderr; a release is never
# guessed available.
#
# Root: GET /v2/<repo>/manifests/<root digest> on ghcr.io, with an anonymous
# pull token, answers 200 and the same digest in Docker-Content-Digest.
# Evidence: the OCI referrers of the root list a Sigstore bundle artifact.
#
# ASSUMPTION: the evidence artifact type below is a placeholder until oyzu
# S2c (#584) publishes real packages and fixes their evidence layout; change
# EVIDENCE_TYPE there. Only ghcr.io OCI routes can be verified here.
set -euo pipefail

candidates=$1
curl=${CURL:-curl}
EVIDENCE_TYPE=${EVIDENCE_TYPE:-application/vnd.dev.sigstore.bundle.v0.3+json}
accept='application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json'

skip() { echo "not advertised: $1@$2: $3" >&2; }

available() { # repository root_digest
  local path=${1#ghcr.io/} token headers refs
  token=$("$curl" -fsS "https://ghcr.io/token?service=ghcr.io&scope=repository:$path:pull" | jq -er .token) || return 1
  headers=$("$curl" -fsSI -H "Authorization: Bearer $token" -H "Accept: $accept" \
    "https://ghcr.io/v2/$path/manifests/$2") || return 1
  printf '%s\n' "$headers" | tr -d '\r' | grep -iqx "docker-content-digest: $2" || return 1
  refs=$("$curl" -fsS -H "Authorization: Bearer $token" -H "Accept: application/vnd.oci.image.index.v1+json" \
    "https://ghcr.io/v2/$path/referrers/$2") || return 1
  printf '%s' "$refs" | jq -e --arg t "$EVIDENCE_TYPE" \
    '[.manifests[]? | select(.artifactType == $t)] | length > 0' > /dev/null
}

out='[]'
while IFS= read -r candidate; do
  package=$(jq -r .package <<< "$candidate")
  version=$(jq -r .version <<< "$candidate")
  root=$(jq -r .root_digest <<< "$candidate")
  repository=$(jq -r '.repository // ""' <<< "$candidate")
  case "$repository" in
    ghcr.io/*) ;;
    *) skip "$package" "$version" "no ghcr.io OCI route to verify"; continue ;;
  esac
  if available "$repository" "$root"; then
    out=$(jq -c --arg p "$package" --arg v "$version" --arg r "$root" \
      '. + [{package: $p, version: $v, root_digest: $r}]' <<< "$out")
  else
    skip "$package" "$version" "release root or evidence is not available"
  fi
done < <(jq -c '.[]' "$candidates")
jq -n --argjson releases "$out" '{releases: $releases}'
