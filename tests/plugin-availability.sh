#!/usr/bin/env bash
# Checks plugin-availability.sh against a fake ghcr.io served from files by a
# fake curl (the script's CURL hook). Covers where evidence is read from:
#   - the referrers API (200),
#   - the referrers tag schema index when the API answers 404,
#   - neither (both 404, or a tag index without the evidence artifact),
#   - errors (5xx, malformed JSON), which fail the script instead.
# Usage: tests/plugin-availability.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
script=$root/.github/scripts/plugin-availability.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

digest=sha256:a4225cd722c100dc0702ce3812f3af251a018f9a3808d146d1e0f1d77d3713f7
hex=${digest#sha256:}
type=application/vnd.dev.sigstore.bundle.v0.3+json
with="{\"manifests\":[{\"artifactType\":\"$type\"}]}"
without='{"manifests":[{"artifactType":"application/vnd.example.sbom+json"}]}'

# Answers from $REG/<name>.code and <name>.body, 404 when absent. Logs every
# URL and its Accept header to $REG/log, and refuses any credential other than
# the anonymous pull token.
cat > "$work/curl" <<'CURL'
#!/usr/bin/env bash
out=/dev/stdout hdr=/dev/null write= accept=
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out=$2; shift ;; -D) hdr=$2; shift ;; -w) write=$2; shift ;;
    -H) case "$2" in Accept:*) accept=$2 ;; esac; shift ;;
    --max-time|--retry) shift ;; http*) url=$1 ;;
  esac
  shift
done
echo "$url | $accept" >> "$REG/log"
case "$url" in
  *"/token?"*) printf '{"token":"t"}' > "$out"; printf 200; exit 0 ;;
  *"/manifests/$ROOT") printf 'HTTP/2 200\r\nDocker-Content-Digest: %s\r\n\r\n' "$ROOT" > "$hdr"; : > "$out"; printf 200; exit 0 ;;
esac
name=none
case "$url" in
  *"/referrers/$ROOT") name=referrers ;;
  *"/manifests/sha256-${ROOT#sha256:}") name=tag ;;
esac
if [ -e "$REG/$name.code" ]; then code=$(cat "$REG/$name.code"); else code=404; fi
if [ -e "$REG/$name.body" ]; then cp "$REG/$name.body" "$out"; else : > "$out"; fi
[ -n "$write" ] && printf '%s' "$code"
exit 0
CURL
chmod +x "$work/curl"

fail() { echo "FAIL: $*" >&2; exit 1; }

printf '[{"package":"p","version":"1.0.0","root_digest":"%s","repository":"ghcr.io/oyzuai/p"}]' "$digest" > "$work/candidates.json"

# run <name> <referrers code|-> <referrers body|-> <tag code|-> <tag body|->
# Sets $status (script exit code) and $verified (count of releases kept).
run() {
  REG=$work/reg; rm -rf "$REG"; mkdir -p "$REG"
  [ "$2" = - ] || printf '%s' "$2" > "$REG/referrers.code"
  [ "$3" = - ] || printf '%s' "$3" > "$REG/referrers.body"
  [ "$4" = - ] || printf '%s' "$4" > "$REG/tag.code"
  [ "$5" = - ] || printf '%s' "$5" > "$REG/tag.body"
  status=0
  REG=$REG ROOT=$digest CURL=$work/curl "$script" "$work/candidates.json" > "$work/out" 2> "$work/err" || status=$?
  verified=-
  [ ! -s "$work/out" ] || verified=$(jq '.releases | length' "$work/out")
}

expect() { # name status verified
  [ "$status" = "$2" ] && [ "$verified" = "$3" ] || { cat "$work/err" >&2; fail "$1: status $status (want $2), kept $verified (want $3)"; }
}

run api 200 "$with" - -
expect "referrers API with evidence" 0 1
grep -q "/manifests/sha256-$hex" "$REG/log" && fail "tag index read although the API answered"

run tag 404 - 200 "$with"
expect "tag schema with evidence" 0 1
grep -F "/manifests/sha256-$hex | Accept: application/vnd.oci.image.index.v1+json" "$REG/log" > /dev/null || fail "tag index not requested as an image index"

run both-404 404 - 404 -
expect "both 404" 0 0
grep -q 'not advertised' "$work/err" || fail "no reason logged"

run tag-no-signature 404 - 200 "$without"
expect "tag index without the signature referrer" 0 0

run tag-empty 404 - 200 '{"manifests":[]}'
expect "empty tag index" 0 0

run api-no-signature 200 "$without" 200 "$with"
expect "API answers, so the tag index is not consulted" 0 0

run api-5xx 503 - - -
expect "referrers 5xx" 1 -
grep -q 'unexpected HTTP 503' "$work/err" || fail "5xx not reported"

run tag-5xx 404 - 502 -
expect "tag index 5xx" 1 -
grep -q 'unexpected HTTP 502' "$work/err" || fail "tag 5xx not reported"

run tag-401 404 - 401 -
expect "tag index 401" 1 -

run api-bad-json 200 'not json' - -
expect "malformed referrers JSON" 1 -
grep -q 'not valid JSON' "$work/err" || fail "malformed JSON not reported"

run tag-bad-json 404 - 200 'not json'
expect "malformed tag index JSON" 1 -

echo "plugin availability checks passed"
