#!/usr/bin/env bash
# Builds the P-002 scorer (release) with the commit and cleanliness of the tree it is built from
# embedded, as a hold-out run requires (`p002 --frozen-commit <sha>` is refused unless the binary
# says it was built from that commit on a clean tree).
#
#   tools/p002_build.sh [extra cargo args]
set -euo pipefail
# The sealed hold-out data is never read: a path argument (not an option) is checked before anything
# runs. Nothing switches this check off.
sealed_args=()
for a in "$@"; do case "$a" in -*) ;; *) sealed_args+=("$a") ;; esac; done
[ "${#sealed_args[@]}" -eq 0 ] ||
    python3 -I "$(dirname "${BASH_SOURCE[0]}")/sealed_guard.py" "${sealed_args[@]}" || exit 2
repo="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
cd "$repo"
commit="$(git rev-parse HEAD)"
if [ -z "$(git status --porcelain)" ]; then dirty=0; else dirty=1; fi
echo "p002_build: commit $commit, dirty=$dirty" >&2
OA_BUILD_COMMIT="$commit" OA_BUILD_DIRTY="$dirty" \
    exec cargo build --release --locked -p oa-track-tools --example p002 "$@"
