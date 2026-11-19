#!/usr/bin/env bash
# Applies the frozen overlay patch (the --digest/--rows flags) to a commit that predates them, or
# refuses. Used by replay_compare.sh; refusing is the only safe answer to any doubt, because a
# patch that lands in the wrong place builds fine and compares the wrong thing (B6, E5).
#
#   tools/replay_overlay_apply.sh <worktree> <commit> <patch> <report>
#
# Checks, in order, each a refusal (exit 2) when it fails:
#   1. every file the patch changes has, at <commit>, the blob the patch's `index` line names as
#      its preimage (a file the patch creates must not exist at <commit>);
#   2. `git apply --check` accepts the patch (git apply allows no fuzz: context must match);
#   3. `git apply` applies it;
#   4. every patched file now has the blob the `index` line names as its postimage.
# <report> receives the sha256 of the patch and of every patched file.
set -euo pipefail

die() {
    echo "replay_overlay_apply: $*" >&2
    exit 2
}

[ $# -eq 4 ] || die "usage: replay_overlay_apply.sh <worktree> <commit> <patch> <report>"
wt="$1"
commit="$2"
patch="$3"
report="$4"
[ -d "$wt" ] || die "no such worktree: $wt"
[ -f "$patch" ] || die "no such patch: $patch"
# git -C changes directory: the patch and the report are named by absolute path.
patch="$(cd "$(dirname "$patch")" && pwd)/$(basename "$patch")"
report="$(cd "$(dirname "$report")" && pwd)/$(basename "$report")"
git -C "$wt" rev-parse --verify --quiet "$commit^{commit}" >/dev/null || die "no such commit: $commit"

# "<path> <preimage> <postimage>" for every file of the patch, from `diff --git` and `index`.
mapfile -t entries < <(awk '
    /^diff --git / { path = $4; sub(/^b\//, "", path) }
    /^index [0-9a-f]+\.\.[0-9a-f]+/ { split($2, ids, "\\.\\."); print path, ids[1], ids[2] }
' "$patch")
[ "${#entries[@]}" -gt 0 ] || die "the patch names no file blobs (no index lines)"

zeros() { [[ "$1" =~ ^0+$ ]]; }

for e in "${entries[@]}"; do
    read -r path pre _ <<<"$e"
    if zeros "$pre"; then
        if git -C "$wt" cat-file -e "$commit:$path" 2>/dev/null; then
            die "$path exists at $commit, but the patch creates it"
        fi
    else
        have="$(git -C "$wt" rev-parse --verify --quiet "$commit:$path" || true)"
        [ -n "$have" ] || die "$path does not exist at $commit, but the patch changes it"
        case "$have" in
        "$pre"*) ;;
        *) die "preimage mismatch for $path at $commit: has blob ${have:0:12}, the patch expects $pre" ;;
        esac
    fi
done

git -C "$wt" apply --check "$patch" || die "git apply --check refused the patch at $commit"
git -C "$wt" apply "$patch" || die "git apply failed at $commit"

for e in "${entries[@]}"; do
    read -r path _ post <<<"$e"
    got="$(git -C "$wt" hash-object -- "$path")"
    case "$got" in
    "$post"*) ;;
    *) die "postimage mismatch for $path: blob ${got:0:12}, the patch produces $post" ;;
    esac
done

{
    echo "overlay patch sha256: $(sha256sum "$patch" | cut -d' ' -f1)"
    echo "applied to commit: $commit"
    for e in "${entries[@]}"; do
        read -r path _ _ <<<"$e"
        echo "patched file sha256: $(sha256sum "$wt/$path" | cut -d' ' -f1)  $path"
    done
} >"$report"
