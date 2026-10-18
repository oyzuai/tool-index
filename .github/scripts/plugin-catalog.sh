#!/usr/bin/env bash
# Failure isolation for the plugin catalog. Runs plugin-catalog-run.sh in its
# own shell and always exits 0: when generation, signing or verification
# fails, plugins.json and its bundle are removed and the release goes out with
# the tools assets alone, so index.json never expires because of plugins.json.
# (A separate process, because `set -e` is ignored inside an `if` condition.)
dir=${RELEASE_DIR:-release}
if bash "$(dirname "$0")/plugin-catalog-run.sh"; then
  echo "plugin catalog: signed and verified"
else
  status=$?
  rm -f "$dir/plugins.json" "$dir/plugins.json.sigstore.json"
  echo "::warning title=Plugin catalog::plugins.json was not produced (exit $status); this release carries the tools assets only"
fi
exit 0
