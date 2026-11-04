#!/usr/bin/env bash
# Deletes this repository's older GitHub Actions caches, keeping the newest of each
# kind. GitHub keeps at most 10 GB of caches and, past that, deletes the least recently
# used, which was the Flatpak's dependency cache (USD, MoonRay...: hours to rebuild),
# because every release's new Cargo.lock left a stale copy of each test cache behind.
#
#   prune-caches.sh <key prefix> <how many "-part"s to strip for the kind>
#
# Caches whose keys are the same once that many trailing "-part"s are stripped are the
# same kind; only the newest of each kind is kept. Needs GH_TOKEN with actions: write.
set -euo pipefail
prefix=$1
strip=$2
gh cache list --key "$prefix" --limit 100 --json id,key,createdAt --jq 'sort_by(.createdAt) | reverse | .[] | "\(.id) \(.key)"' |
  while read -r id key; do
    kind=$key
    for _ in $(seq 1 "$strip"); do kind=${kind%-*}; done
    if grep -qxF "$kind" "${RUNNER_TEMP:-/tmp}/kept-caches" 2>/dev/null; then
      echo "Deleting older cache $key"
      gh cache delete "$id" || echo "::warning::Couldn't delete cache $key"
    else
      echo "$kind" >> "${RUNNER_TEMP:-/tmp}/kept-caches"
      echo "Keeping $key"
    fi
  done
