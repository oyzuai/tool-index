#!/usr/bin/env bash
# Removes the oyzuai/oyzu credential the `oyzu-read` action added to the
# global git config.
set -euo pipefail
git config --global --name-only --get-regexp '^url\..*oyzuai/oyzu\.insteadof$' |
  while read -r key; do git config --global --unset-all "$key"; done
