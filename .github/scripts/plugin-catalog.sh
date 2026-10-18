#!/usr/bin/env bash
# Failure isolation for the plugin catalog. Runs plugin-catalog-run.sh in its
# own shell, bounded in time, and always exits 0: when generation, signing or
# verification fails or hangs, plugins.json and its bundle are removed and the
# release goes out with the tools assets alone, so index.json never expires
# because of plugins.json.
# (A separate process, because `set -e` is ignored inside an `if` condition.
# `timeout` signals the whole process group, so a hung cosign or curl dies
# with it; a step-level timeout would fail the job instead.)
dir=${RELEASE_DIR:-release}
if timeout --kill-after=15 "${PLUGIN_TIMEOUT:-600}" bash "$(dirname "$0")/plugin-catalog-run.sh"; then
  echo "plugin catalog: signed and verified"
else
  status=$?
  rm -f "$dir/plugins.json" "$dir/plugins.json.sigstore.json"
  echo "::warning title=Plugin catalog::plugins.json was not produced (exit $status, 124 is a timeout); this release carries the tools assets only"
fi
exit 0
