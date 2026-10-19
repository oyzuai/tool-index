#!/usr/bin/env bash
# Usage: plugin-availability.sh candidates.json > verified.json
#
# Catalog-last. Prints {"releases": [...]} holding each candidate release
# (from `tool-index-release plugins candidates`) whose OCI release root is
# fetchable anonymously by digest and whose evidence is attached to it.
# Anything else is left out, with the reason on stderr; a release is never
# guessed available.
#
# Root: HEAD /v2/<repo>/manifests/<root digest> on ghcr.io, with an anonymous
# pull token, answers 200 and the same digest in Docker-Content-Digest.
# A release is left out only on a definite 404 or a missing evidence entry;
# every other error (timeouts, 5xx, 401/403) fails the script instead.
# Evidence: the OCI referrers of the root list a Sigstore bundle artifact. They
# are read from the referrers API; when it answers 404 (ghcr.io does), from the
# OCI referrers tag schema index at manifests/sha256-<hex of the root digest>.
# A 404 there too means no evidence. Both are checked with the same filter.
#
# ASSUMPTION: the evidence artifact type below is a placeholder until oyzu
# S2c (#584) publishes real packages and fixes their evidence layout; change
# EVIDENCE_TYPE there. Only ghcr.io OCI routes can be verified here.
set -euo pipefail

candidates=$1
curl=${CURL:-curl}
EVIDENCE_TYPE=${EVIDENCE_TYPE:-application/vnd.dev.sigstore.bundle.v0.3+json}
accept='application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json'
index_type=application/vnd.oci.image.index.v1+json

skip() { echo "not advertised: $1@$2: $3" >&2; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# GET/HEAD with a time bound and retries. Prints the HTTP status; the body is
# in $tmp/body and the headers in $tmp/headers. A transport error (timeout,
# reset, DNS) ends the whole script with a failure: only a definite answer may
# decide that a release is unavailable, so a blip makes the catalog run fail
# (plugins absent from this release) and never publishes a catalog that lacks
# packages.
fetch() {
  "$curl" -sS --max-time 30 --retry 3 --retry-all-errors -o "$tmp/body" -D "$tmp/headers" \
    -w '%{http_code}' "$@" || { echo "network error talking to ghcr.io" >&2; exit 1; }
}

# Returns 0 when available, 1 when GHCR answers 404 (unknown manifest or no
# referrers or tag index); any other answer ends the script with a failure.
available() { # repository root_digest
  local path=${1#ghcr.io/} token code
  code=$(fetch "https://ghcr.io/token?service=ghcr.io&scope=repository:$path:pull")
  [ "$code" = 200 ] && token=$(jq -er .token "$tmp/body") || { echo "no pull token for $path (HTTP $code)" >&2; exit 1; }
  code=$(fetch -I -H "Authorization: Bearer $token" -H "Accept: $accept" "https://ghcr.io/v2/$path/manifests/$2")
  case "$code" in
    200) ;;
    404) return 1 ;;
    *) echo "unexpected HTTP $code for $path manifest" >&2; exit 1 ;;
  esac
  tr -d '\r' < "$tmp/headers" | grep -iqx "docker-content-digest: $2" || return 1
  # The referrers API; a registry without it answers 404, and the client then
  # reads the referrers tag schema index at manifests/sha256-<hex> instead.
  code=$(fetch -H "Authorization: Bearer $token" -H "Accept: $index_type" \
    "https://ghcr.io/v2/$path/referrers/$2")
  case "$code" in
    200) ;;
    404)
      code=$(fetch -H "Authorization: Bearer $token" -H "Accept: $index_type" \
        "https://ghcr.io/v2/$path/manifests/sha256-${2#sha256:}")
      case "$code" in
        200) ;;
        404) return 1 ;;
        *) echo "unexpected HTTP $code for $path referrers tag" >&2; exit 1 ;;
      esac ;;
    *) echo "unexpected HTTP $code for $path referrers" >&2; exit 1 ;;
  esac
  # A body that is not JSON is an error, not "no evidence": jq inside `if
  # available` would otherwise turn it into a silent skip.
  jq -e type "$tmp/body" > /dev/null 2>&1 || { echo "referrers of $path are not valid JSON" >&2; exit 1; }
  jq -e --arg t "$EVIDENCE_TYPE" '[.manifests[]? | select(.artifactType == $t)] | length > 0' "$tmp/body" > /dev/null
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
