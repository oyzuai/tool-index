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
out=/dev/stdout hdr=/dev/null write= accept= glob=0
while [ $# -gt 0 ]; do
  case "$1" in
    -g|-sSg) glob=1 ;;
    -o) out=$2; shift ;; -D) hdr=$2; shift ;; -w) write=$2; shift ;;
    -H) case "$2" in Accept:*) accept=$2 ;; esac; shift ;;
    --max-time|--retry) shift ;; http*) url=$1 ;;
  esac
  shift
done
echo "$url | $accept | g=$glob" >> "$REG/log"
case "$url" in
  *"/token?"*) printf '{"token":"t"}' > "$out"; printf 200; exit 0 ;;
  *"/manifests/$ROOT")
    if [ -e "$REG/root.stale" ]; then
      printf 'HTTP/2 503\r\nDocker-Content-Digest: %s\r\n\r\nHTTP/2 200\r\nDocker-Content-Digest: sha256:%064d\r\n\r\n' "$ROOT" 0 > "$hdr"
    else
      printf 'HTTP/2 200\r\nDocker-Content-Digest: %s\r\n\r\n' "$ROOT" > "$hdr"
    fi
    : > "$out"; printf 200; exit 0 ;;
esac
name=none
case "$url" in
  *"/referrers/$ROOT") name=referrers ;;
  *"/referrers/$ROOT?"*) name=referrers2 ;;
  *"/manifests/sha256-${ROOT#sha256:}") name=tag ;;
  *"/manifests/sha256:"*) name=m-${url##*/manifests/sha256:} ;;
esac
[ -e "$REG/$name.neterr" ] && exit 28
[ -e "$REG/$name.stale" ] && printf '%s 503\r\nLink: %s\r\n\r\n' "$([ -e "$REG/lower" ] && echo http/2 || echo HTTP/2)" "$(cat "$REG/$name.stale")" > "$hdr" || : > "$hdr"
printf '%s 200\r\n' "$([ -e "$REG/lower" ] && echo http/2 || echo HTTP/2)" >> "$hdr"
[ -e "$REG/$name.link" ] && printf '%s: %s\r\n' "$(cat "$REG/linkname" 2>/dev/null || echo Link)" "$(cat "$REG/$name.link")" >> "$hdr"
printf '\r\n' >> "$hdr"
if [ -e "$REG/$name.code" ]; then code=$(cat "$REG/$name.code"); else code=404; fi
if [ -e "$REG/$name.body" ]; then cp "$REG/$name.body" "$out"; else : > "$out"; fi
# Evidence on the Nth page after the first, whatever the page URL is.
if [ "$name" = referrers2 ] && [ -e "$REG/referrers2.evidence_at" ] \
  && [ "$(grep -c '/referrers/.*?' "$REG/log")" = "$(cat "$REG/referrers2.evidence_at")" ]; then
  cp "$REG/referrers2.evidence" "$out"
fi
[ -n "$write" ] && printf '%s' "$code"
# A transfer cut after the headers: the status and a body arrive, curl fails.
[ -e "$REG/$name.cut" ] && exit 18
exit 0
CURL
chmod +x "$work/curl"

fail() { echo "FAIL: $*" >&2; exit 1; }

printf '[{"package":"p","version":"1.0.0","root_digest":"%s","repository":"ghcr.io/oyzuai/p"}]' "$digest" > "$work/candidates.json"

# put <name> <code> [body]: the fake registry's answer for <name>.
put() { printf '%s' "$2" > "$REG/$1.code"; [ $# -lt 3 ] || printf '%s' "$3" > "$REG/$1.body"; }
sha() { printf '%s' "$1" | sha256sum | cut -d' ' -f1; }

# run: runs the script against $REG; sets $status and $verified (releases kept,
# - when there is no output).
run() {
  status=0
  PATH=${PATHPREFIX:+$PATHPREFIX:}$PATH REG=$REG ROOT=$digest CURL=$work/curl "$script" "$work/candidates.json" > "$work/out" 2> "$work/err" || status=$?
  verified=-
  [ ! -s "$work/out" ] || verified=$(jq '.releases | length' "$work/out")
}
reg() { REG=$work/reg; rm -rf "$REG"; mkdir -p "$REG"; }

expect() { # name status verified
  [ "$status" = "$2" ] && [ "$verified" = "$3" ] || { cat "$work/err" >&2; fail "$1: status $status (want $2), kept $verified (want $3)"; }
}
errs() { grep -q "$1" "$work/err" || { cat "$work/err" >&2; fail "$name: no '$1' in the error"; }; }

# A referrer manifest for the evidence type, its digest, and a tag index of it.
manifest() { # subject -> sets $m (manifest body) and $md (its digest)
  m="{\"schemaVersion\":2,\"artifactType\":\"$type\",\"subject\":{\"digest\":\"$1\"}}"
  md=sha256:$(sha "$m")
}
index_of() { printf '{"manifests":[{"artifactType":"%s","digest":"%s"}]}' "$type" "$1"; }
good_tag() { manifest "$digest"; put tag 200 "$(index_of "$md")"; put "m-${md#sha256:}" 200 "$m"; }

name="referrers API with evidence"
reg; put referrers 200 "$with"; run
expect "$name" 0 1
grep -q "/manifests/sha256-$hex" "$REG/log" && fail "tag index read although the API answered"

name="tag schema with evidence"
reg; put referrers 404; good_tag; run
expect "$name" 0 1
grep -F "/manifests/sha256-$hex | Accept: application/vnd.oci.image.index.v1+json" "$REG/log" > /dev/null || fail "tag index not requested as an image index"
grep -F "/manifests/$md | Accept: application/vnd.oci.image.manifest.v1+json" "$REG/log" > /dev/null || fail "referrer manifest not fetched by digest"

name="both 404"
reg; put referrers 404; put tag 404; run
expect "$name" 0 0
errs 'not advertised'

name="tag index without the signature referrer"
reg; put referrers 404; put tag 200 "$without"; run
expect "$name" 0 0

name="empty tag index"
reg; put referrers 404; put tag 200 '{"manifests":[]}'; run
expect "$name" 0 0

name="API answers without evidence, tag index not consulted"
reg; put referrers 200 "$without"; good_tag; run
expect "$name" 0 0

name="dangling tag index entry (referrer manifest missing)"
reg; put referrers 404; manifest "$digest"; put tag 200 "$(index_of "$md")"; run
expect "$name" 0 0

name="referrer manifest with another subject"
reg; put referrers 404; manifest "sha256:$(sha other)"; put tag 200 "$(index_of "$md")"; put "m-${md#sha256:}" 200 "$m"; run
expect "$name" 0 0

name="referrer manifest without a subject"
reg; put referrers 404; m="{\"schemaVersion\":2,\"artifactType\":\"$type\"}"; md=sha256:$(sha "$m")
put tag 200 "$(index_of "$md")"; put "m-${md#sha256:}" 200 "$m"; run
expect "$name" 0 0

name="referrer manifest of another artifact type"
reg; put referrers 404; m="{\"schemaVersion\":2,\"artifactType\":\"application/vnd.example.sbom+json\",\"subject\":{\"digest\":\"$digest\"}}"; md=sha256:$(sha "$m")
put tag 200 "$(index_of "$md")"; put "m-${md#sha256:}" 200 "$m"; run
expect "$name" 0 0

name="referrer manifest that does not hash to its descriptor"
reg; put referrers 404; manifest "$digest"; put tag 200 "$(index_of "sha256:$(sha forged)")"; put "m-$(sha forged)" 200 "$m"; run
expect "$name" 0 0

name="descriptor digest that is not a digest"
reg; put referrers 404; put tag 200 "$(index_of '../../x')"; run
expect "$name" 0 0

name="second descriptor holds the evidence"
reg; put referrers 404; manifest "$digest"; good="$md"; gm="$m"; manifest "sha256:$(sha other)"
put tag 200 "{\"manifests\":[{\"artifactType\":\"$type\",\"digest\":\"$md\"},{\"artifactType\":\"$type\",\"digest\":\"$good\"}]}"
put "m-${md#sha256:}" 200 "$m"; put "m-${good#sha256:}" 200 "$gm"; run
expect "$name" 0 1

name="referrers 5xx"
reg; put referrers 503; run
expect "$name" 1 -; errs 'unexpected HTTP 503'

name="tag index 5xx"
reg; put referrers 404; put tag 502; run
expect "$name" 1 -; errs 'unexpected HTTP 502'

name="tag index 401"
reg; put referrers 404; put tag 401; run
expect "$name" 1 -

name="tag index redirect"
reg; put referrers 404; put tag 307; run
expect "$name" 1 -; errs 'unexpected HTTP 307'

name="tag index transport error"
reg; put referrers 404; touch "$REG/tag.neterr"; run
expect "$name" 1 -; errs 'network error'

name="referrer manifest 5xx"
reg; put referrers 404; manifest "$digest"; put tag 200 "$(index_of "$md")"; put "m-${md#sha256:}" 500; run
expect "$name" 1 -; errs 'unexpected HTTP 500'

name="referrer manifest transport error"
reg; put referrers 404; manifest "$digest"; put tag 200 "$(index_of "$md")"; touch "$REG/m-${md#sha256:}.neterr"; run
expect "$name" 1 -; errs 'network error'

name="malformed referrers JSON"
reg; put referrers 200 'not json'; run
expect "$name" 1 -; errs 'not a single JSON object'

name="several JSON values in the referrers body"
reg; put referrers 200 "$with $without"; run
expect "$name" 1 -; errs 'not a single JSON object'

name="malformed tag index JSON"
reg; put referrers 404; put tag 200 'not json'; run
expect "$name" 1 -

name="evidence on the second referrers page"
reg; put referrers 200 "$without"; echo "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"next\"" > "$REG/referrers.link"
put referrers2 200 "$with"; run
expect "$name" 0 1

name="no evidence on any referrers page"
reg; put referrers 200 "$without"; echo "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"next\"" > "$REG/referrers.link"
put referrers2 200 "$without"; run
expect "$name" 0 0

name="referrers Link to another route"
reg; put referrers 200 "$without"; echo '<https://evil.example/x>; rel="next"' > "$REG/referrers.link"; run
expect "$name" 1 -; errs 'unexpected referrers Link'

name="second referrers page 5xx"
reg; put referrers 200 "$without"; echo "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"next\"" > "$REG/referrers.link"
put referrers2 500; run
expect "$name" 1 -


name="tag index cut after the headers"
reg; put referrers 404; good_tag; touch "$REG/tag.cut"; run
expect "$name" 1 -; errs 'network error'

name="referrer manifest cut after the headers"
reg; put referrers 404; good_tag; touch "$REG/m-${md#sha256:}.cut"; run
expect "$name" 1 -; errs 'network error'

name="referrers page cut after the headers"
reg; put referrers 200 "$with"; touch "$REG/referrers.cut"; run
expect "$name" 1 -; errs 'network error'

name="tag index entry that is not an object"
reg; put referrers 404; put tag 200 '{"manifests":["x"]}'; run
expect "$name" 1 -; errs 'malformed entry'

name="referrers entry that is not an object"
reg; put referrers 200 '{"manifests":["x"]}'; run
expect "$name" 1 -; errs 'malformed entry'

name="descriptor digest that is a glob"
reg; put referrers 404; put tag 200 "$(index_of '*')"; run
expect "$name" 0 0
grep -q 'manifests/\*' "$REG/log" && fail "glob digest reached the network"

name="fifty descriptors in the tag index"
reg; put referrers 404; manifest "$digest"
list=; for i in $(seq 1 49); do list="$list{\"artifactType\":\"$type\",\"digest\":\"sha256:$(sha "x$i")\"},"; done
put tag 200 "{\"manifests\":[$list{\"artifactType\":\"$type\",\"digest\":\"$md\"}]}"; put "m-${md#sha256:}" 200 "$m"; run
expect "$name" 0 1

name="fifty-one descriptors in the tag index"
reg; put referrers 404; manifest "$digest"
list=; for i in $(seq 1 50); do list="$list{\"artifactType\":\"$type\",\"digest\":\"sha256:$(sha "x$i")\"},"; done
put tag 200 "{\"manifests\":[$list{\"artifactType\":\"$type\",\"digest\":\"$md\"}]}"; put "m-${md#sha256:}" 200 "$m"; run
expect "$name" 1 -; errs 'too many referrers'

# Referrers paging: every page links to the next; evidence appears on page N.
paged() { # link header value, evidence page (0: never), [header name]
  reg; [ -z "${3:-}" ] || printf '%s' "$3" > "$REG/linkname"; put referrers 200 "$without"; printf '%s' "$1" > "$REG/referrers.link"; printf '%s' "$1" > "$REG/referrers2.link"
  put referrers2 200 "$without"
  [ "$2" = 0 ] || { printf '%s' "$2" > "$REG/referrers2.evidence_at"; printf '%s' "$with" > "$REG/referrers2.evidence"; }
  run
}
next="</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"next\""

name="evidence on the 50th referrers page"
paged "$next" 49
expect "$name" 0 1

name="evidence on the 51st referrers page"
paged "$next" 50
expect "$name" 1 -; errs 'too many referrers pages'

name="rel with several tokens"
paged "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"prev next\"" 1
expect "$name" 0 1

name="rel in capitals, header name in capitals"
paged "</v2/oyzuai/p/referrers/$digest?last=x>; REL=NEXT" 1
expect "$name" 0 1

name="header name in capitals"
paged "$next" 1 LINK
expect "$name" 0 1

name="next after another link value"
paged "</v2/oyzuai/p/referrers/$digest?last=a>; rel=\"prev\", </v2/oyzuai/p/referrers/$digest?last=x>; rel=\"next\"" 1
expect "$name" 0 1

name="rel that only starts with next"
paged "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"nextpage\"" 1
expect "$name" 0 0

name="only a prev link"
paged "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"prev\"" 1
expect "$name" 0 0

name="page URL with glob characters"
paged "</v2/oyzuai/p/referrers/$digest?last=[1]>; rel=\"next\"" 1
expect "$name" 0 1
grep -v 'g=1' "$REG/log" | grep -q . && fail "a request ran without -g"

name="next link first, another link after it"
paged "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"next\", </v2/oyzuai/p/referrers/$digest?last=a>; rel=\"prev\"" 1
expect "$name" 0 1

name="unquoted rel=next followed by a comma"
paged "</v2/oyzuai/p/referrers/$digest?last=x>; rel=next, </v2/oyzuai/p/referrers/$digest?last=a>; rel=prev" 1
expect "$name" 0 1

name="rel=next inside a quoted parameter"
paged "</v2/oyzuai/p/referrers/$digest?last=x>; title=\"x; rel=next\"" 1
expect "$name" 0 0

name="comma inside a quoted parameter before rel"
paged "</v2/oyzuai/p/referrers/$digest?last=x>; title=\"a, <b>\"; rel=\"next\"" 1
expect "$name" 0 1

name="Link in the header block of a retried attempt only"
reg; put referrers 200 "$without"; echo "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"next\"" > "$REG/referrers.stale"
put referrers2 200 "$with"; run
expect "$name" 0 0
grep -q 'referrers/.*?' "$REG/log" && fail "followed a Link from an earlier attempt"

name="root digest in the header block of a retried attempt only"
reg; put referrers 200 "$with"; touch "$REG/root.stale"; run
expect "$name" 0 0

name="lowercase status lines in the header blocks"
reg; touch "$REG/lower"; put referrers 200 "$without"; echo "</v2/oyzuai/p/referrers/$digest?last=x>; rel=\"next\"" > "$REG/referrers.stale"
put referrers2 200 "$with"; run
expect "$name" 0 0

name="header reduction fails"
mkdir -p "$work/badawk"; printf '#!/bin/sh\nexit 2\n' > "$work/badawk/awk"; chmod +x "$work/badawk/awk"
reg; put referrers 200 "$with"; PATHPREFIX=$work/badawk run; unset PATHPREFIX
expect "$name" 1 -; errs 'cannot read the response headers'

# The script's own inputs.
cand=$work/candidates.json
name="root digest that is not a digest"
printf '[{"package":"p","version":"1.0.0","root_digest":"sha256:ABC","repository":"ghcr.io/oyzuai/p"}]' > "$cand"
reg; run
expect "$name" 1 -; errs 'invalid root digest'

name="repository with a path trick"
printf '[{"package":"p","version":"1.0.0","root_digest":"%s","repository":"ghcr.io/oyzuai/../p"}]' "$digest" > "$cand"
reg; run
expect "$name" 1 -; errs 'invalid repository'

name="repository with an uppercase letter"
printf '[{"package":"p","version":"1.0.0","root_digest":"%s","repository":"ghcr.io/OyzuAI/p"}]' "$digest" > "$cand"
reg; run
expect "$name" 1 -; errs 'invalid repository'
[ ! -e "$REG/log" ] || fail "invalid input reached the network"

name="candidates file that is not JSON"
printf 'not json' > "$cand"; reg; run
expect "$name" 1 -; errs 'exactly one JSON array'

name="candidates file that is an object"
printf '{"package":"p"}' > "$cand"; reg; run
expect "$name" 1 -; errs 'exactly one JSON array'

name="empty candidates list"
printf '[]' > "$cand"; reg; run
expect "$name" 0 0

name="empty candidates file"
: > "$cand"; reg; run
expect "$name" 1 -; errs 'exactly one JSON array'

name="whitespace-only candidates file"
printf ' \n\n' > "$cand"; reg; run
expect "$name" 1 -; errs 'exactly one JSON array'

name="two JSON arrays in the candidates file"
printf '[] []' > "$cand"; reg; run
expect "$name" 1 -; errs 'exactly one JSON array'

echo "plugin availability checks passed"
