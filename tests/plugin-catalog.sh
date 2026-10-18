#!/usr/bin/env bash
# End-to-end check of the release workflow's plugin steps without Sigstore or
# the network: a fake cosign writes a v0.3 DSSE bundle around the statement it
# is given, and a fake curl answers for GHCR. Proves that
#   - an empty source yields a valid plugins.json beside index.json,
#   - an advertised release needs its root and evidence to be available,
#   - a plugin failure leaves the tools release intact.
# Usage: tests/plugin-catalog.sh [path to tool-index-release]
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
tool=$(realpath "${1:-$root/release-tool/target/debug/tool-index-release}")
scripts=$root/.github/scripts
fixtures=$root/tests/fixtures
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cat > "$work/cosign" <<'EOF'
#!/usr/bin/env bash
# attest-blob --yes --statement S --bundle B FILE | verify-blob-attestation ...
case "$1" in
  attest-blob)
    while [ $# -gt 0 ]; do
      case "$1" in --statement) statement=$2; shift ;; --bundle) bundle=$2; shift ;; esac
      shift
    done
    printf '{"mediaType":"application/vnd.dev.sigstore.bundle.v0.3+json","dsseEnvelope":{"payloadType":"application/vnd.in-toto+json","payload":"%s"}}' \
      "$(base64 -w0 "$statement")" > "$bundle" ;;
  verify-blob-attestation) [ -z "${FAKE_VERIFY_FAILS:-}" ] ;;
esac
EOF
chmod +x "$work/cosign"

# Answers like ghcr.io for the one release in plugins.go-preview.toml.
cat > "$work/curl" <<'EOF'
#!/usr/bin/env bash
args="$*"
root=sha256:a4225cd722c100dc0702ce3812f3af251a018f9a3808d146d1e0f1d77d3713f7
case "$args" in
  *"/token?"*) echo '{"token":"t"}' ;;
  *"/manifests/$root"*) [ -n "${FAKE_NO_ROOT:-}" ] && exit 22; printf 'HTTP/2 200\r\nDocker-Content-Digest: %s\r\n\r\n' "$root" ;;
  *"/referrers/$root"*) [ -n "${FAKE_NO_EVIDENCE:-}" ] && { echo '{"manifests":[]}'; exit 0; }
    echo '{"manifests":[{"artifactType":"application/vnd.dev.sigstore.bundle.v0.3+json"}]}' ;;
  *) exit 22 ;;
esac
EOF
chmod +x "$work/curl"

fail() { echo "FAIL: $*" >&2; exit 1; }

# A tools release directory: stamped index.json + fake-signed bundle.
setup() {
  rm -rf "$work/release"; mkdir -p "$work/release"
  "$tool" stamp --index "$fixtures/sample-index.json" --out "$work/release/index.json" \
    --now 2026-10-19T06:00:00Z --config "$root/config" > /dev/null
  (cd "$work/release" && "$work/cosign" attest-blob --yes \
    --statement <(jq -n --arg d "$(sha256sum index.json | cut -d' ' -f1)" \
      '{subject:[{name:"index.json",digest:{sha256:$d}}]}') \
    --bundle index.json.sigstore.json index.json)
  echo notice > "$work/release/NOTICE"
  command -v zstd > /dev/null && zstd -19 -q -o "$work/release/index.json.zst" "$work/release/index.json" || true
}

run_plugins() { # source file
  RELEASE_DIR=$work/release RELEASE_TOOL=$tool PLUGIN_SOURCE=$1 COSIGN=$work/cosign \
    CURL=$work/curl "$scripts/plugin-catalog.sh" > "$work/log" 2>&1 || fail "wrapper exited non-zero"
}

plan() { "$tool" release --dir "$work/release" --dry-run --now 2026-10-19T07:00:00Z; }

# 1. Empty source: a valid, signed, empty catalog with the index's serial.
setup
run_plugins "$root/config/plugins.toml"
[ -f "$work/release/plugins.json" ] && [ -f "$work/release/plugins.json.sigstore.json" ] || { cat "$work/log"; fail "no plugins.json"; }
[ "$(jq -c .content.packages "$work/release/plugins.json")" = '[]' ] || fail "catalog not empty"
[ "$(jq .serial "$work/release/plugins.json")" = "$(jq .serial "$work/release/index.json")" ] || fail "serial differs"
plan | jq -e '.plugins.published == true and ([.assets[].name] | index("plugins.json") != null)' > /dev/null || fail "plan omits plugins"

# The canonical JSON Schema accepts what the tool wrote. Set
# REQUIRE_SCHEMA_CHECK=1 (CI does) to fail when jsonschema is not installed.
validate() {
  if python3 -c 'import jsonschema, referencing' 2> /dev/null; then
    python3 "$root/tests/validate-plugin-schema.py" "$@" || fail "schema validation"
  elif [ -n "${REQUIRE_SCHEMA_CHECK:-}" ]; then
    fail "python jsonschema is required"
  else
    echo "skipping schema validation: python jsonschema not installed" >&2
  fi
}
validate "$work/release/plugins.json"

# 2. Re-signing unchanged content keeps content_digest and moves the envelope.
digest=$(jq -r .content_digest "$work/release/plugins.json")
setup
"$tool" stamp --index "$fixtures/sample-index.json" --published "$work/release/index.json" \
  --out "$work/release/index.json" --now 2026-10-20T06:00:00Z --config "$root/config" > /dev/null
run_plugins "$root/config/plugins.toml"
[ "$(jq -r .content_digest "$work/release/plugins.json")" = "$digest" ] || fail "digest moved"

# 3. A listed release is advertised only when its root and evidence are available.
source=$fixtures/plugins-v1alpha1/plugins.go-preview.toml
setup; run_plugins "$source"
[ "$(jq '.content.packages | length' "$work/release/plugins.json")" = 1 ] || { cat "$work/log"; fail "available release not advertised"; }
validate "$work/release/plugins.json"
for missing in FAKE_NO_ROOT FAKE_NO_EVIDENCE; do
  setup; env "$missing=1" bash -c "$(declare -f run_plugins fail); work=$work root=$root tool=$tool scripts=$scripts; run_plugins $source"
  [ "$(jq '.content.packages | length' "$work/release/plugins.json")" = 0 ] || fail "$missing: advertised anyway"
  grep -q 'not advertised' "$work/log" || fail "$missing: no reason logged"
done

# 4. Failure isolation: a broken source, or a signature that does not verify,
# leaves no plugin assets and the tools release still plans.
setup
printf 'authority = "oyzu"\nmirrors = []\n' > "$work/bad.toml"
run_plugins "$work/bad.toml"
[ ! -e "$work/release/plugins.json" ] && [ ! -e "$work/release/plugins.json.sigstore.json" ] || fail "partial plugin files left"
grep -q '::warning' "$work/log" || fail "no warning"
plan | jq -e '.plugins.published == false and (.missing | index("index.json.sigstore.json") == null) and .serial == 1' > /dev/null || fail "tools release blocked"

setup
FAKE_VERIFY_FAILS=1 run_plugins "$root/config/plugins.toml"
[ ! -e "$work/release/plugins.json" ] || fail "unverified catalog left"
plan | jq -e '.plugins.published == false' > /dev/null || fail "tools release blocked"
echo "plugin catalog workflow checks passed"
