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
# A 404 there too means no evidence. Both use the same artifactType match; the
# tag index, which nothing vouches for, must also lead to a referrer manifest
# whose subject is the root. Referrers pages are followed through Link.
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

# The script's own inputs reach URLs and a grep pattern, so they are checked
# against the shapes OCI allows; anything else ends the script.
digest_re='^sha256:[0-9a-f]{64}$'
repo_re='^[a-z0-9]+([._-][a-z0-9]+)*(/[a-z0-9]+([._-][a-z0-9]+)*)*$'
index_max_pages=50

# The body fetched last must be exactly one JSON object: jq on its own would
# take the last of several values, and a body that is not JSON would turn into a
# silent "no evidence" inside `if available`.
json_object() { # what
  jq -se 'length == 1 and (.[0] | type) == "object"' "$tmp/body" > /dev/null 2>&1 \
    || { echo "$1 is not a single JSON object" >&2; exit 1; }
}

# The referrers tag schema index is writable by anyone who can push a tag, so
# unlike the referrers API it does not vouch for its entries. A descriptor of
# the evidence type counts only when the referrer manifest it names exists in
# this repository, hashes to that digest, and has the root as its subject.
# Returns 0 for evidence, 1 for none; any other answer ends the script.
referrer_attached() { # path token root_digest
  local path=$1 token=$2 root=$3 code digests d
  code=$(fetch -H "Authorization: Bearer $token" -H "Accept: $index_type" \
    "https://ghcr.io/v2/$path/manifests/sha256-${root#sha256:}")
  case "$code" in
    200) ;;
    404) return 1 ;;
    *) echo "unexpected HTTP $code for $path referrers tag" >&2; exit 1 ;;
  esac
  json_object "referrers tag index of $path"
  digests=$(jq -r --arg t "$EVIDENCE_TYPE" '.manifests[]? | select(.artifactType == $t) | .digest' "$tmp/body")
  for d in $digests; do
    [[ $d =~ $digest_re ]] || continue
    code=$(fetch -H "Authorization: Bearer $token" -H "Accept: application/vnd.oci.image.manifest.v1+json" \
      "https://ghcr.io/v2/$path/manifests/$d")
    case "$code" in
      200) ;;
      404) continue ;;
      *) echo "unexpected HTTP $code for $path referrer manifest" >&2; exit 1 ;;
    esac
    [ "sha256:$(sha256sum "$tmp/body" | cut -d' ' -f1)" = "$d" ] || continue
    if jq -e --arg r "$root" --arg t "$EVIDENCE_TYPE" \
      '.subject.digest == $r and .artifactType == $t' "$tmp/body" > /dev/null 2>&1; then
      return 0
    fi
  done
  return 1
}

# Returns 0 when available, 1 when GHCR answers 404 (unknown manifest or no
# referrers or tag index); any other answer ends the script with a failure.
available() { # repository root_digest
  local path=${1#ghcr.io/} root=$2 token code url next pages=0
  [[ $root =~ $digest_re ]] || { echo "invalid root digest: $root" >&2; exit 1; }
  [[ $path =~ $repo_re ]] || { echo "invalid repository: $1" >&2; exit 1; }
  code=$(fetch "https://ghcr.io/token?service=ghcr.io&scope=repository:$path:pull")
  [ "$code" = 200 ] && token=$(jq -er .token "$tmp/body") || { echo "no pull token for $path (HTTP $code)" >&2; exit 1; }
  code=$(fetch -I -H "Authorization: Bearer $token" -H "Accept: $accept" "https://ghcr.io/v2/$path/manifests/$root")
  case "$code" in
    200) ;;
    404) return 1 ;;
    *) echo "unexpected HTTP $code for $path manifest" >&2; exit 1 ;;
  esac
  tr -d '\r' < "$tmp/headers" | grep -iqx "docker-content-digest: $root" || return 1
  # The referrers API, following its Link rel=next pages; a registry without it
  # answers 404 on the first page, and the client then reads the referrers tag
  # schema index instead.
  url="https://ghcr.io/v2/$path/referrers/$root"
  while :; do
    code=$(fetch -H "Authorization: Bearer $token" -H "Accept: $index_type" "$url")
    case "$code" in
      200) ;;
      404)
        [ "$pages" = 0 ] && { referrer_attached "$path" "$token" "$root"; return; }
        echo "unexpected HTTP 404 for $path referrers page" >&2; exit 1 ;;
      *) echo "unexpected HTTP $code for $path referrers" >&2; exit 1 ;;
    esac
    json_object "referrers of $path"
    jq -e --arg t "$EVIDENCE_TYPE" '[.manifests[]? | select(.artifactType == $t)] | length > 0' "$tmp/body" > /dev/null && return 0
    next=$(tr -d '\r' < "$tmp/headers" | sed -n 's/^[Ll][Ii][Nn][Kk]:.*<\([^>]*\)>[^,]*rel="\{0,1\}next"\{0,1\}.*/\1/p' | head -n 1)
    [ -n "$next" ] || return 1
    case "$next" in
      /v2/"$path"/referrers/"$root"\?*) ;;
      *) echo "unexpected referrers Link for $path: $next" >&2; exit 1 ;;
    esac
    pages=$((pages + 1))
    [ "$pages" -lt "$index_max_pages" ] || { echo "too many referrers pages for $path" >&2; exit 1; }
    url="https://ghcr.io$next"
  done
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
