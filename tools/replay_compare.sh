#!/usr/bin/env bash
# Compares the tracker output of two commits on one recorded clip.
#
#   tools/replay_compare.sh <commitA> <commitB> <clip.oaf> <outdir> [scratchdir] [--expect <file>]
#
# Each commit is built (oa-track-run, release) in its own clean detached worktree with its own
# target directory, one build at a time, and run with --no-score --digest --rows. oa-replay-diff
# then compares the two outputs. The worktrees and target directories are removed at the end;
# the reports stay in <outdir>. Exit status is oa-replay-diff's (0 identical, 1 different,
# 2 error). Only commit hashes or refs are accepted: never a working tree.
#
# Environment:
#   REPLAY_COMPARE_GUARD      command that wraps every build and run (default: a systemd
#                             scope limited to $GUARD_MEM, 6G); set it to the empty string to
#                             run unwrapped
#   REPLAY_COMPARE_TOOLCHAIN  rustup toolchain (default 1.89.0, the pinned one)
#   CARGO_BUILD_JOBS          default 2
#   REPLAY_COMPARE_KEEP_ROWS  1 keeps the two rows files in <outdir>
#
# Every cargo and rustc call (both builds, the diff tool's build, the toolchain query) runs under
# `env -i` with only PATH, HOME, RUSTUP_HOME, CARGO_HOME and TMPDIR (those that are set) and the
# two variables this script sets itself (CARGO_BUILD_JOBS, CARGO_TARGET_DIR), and with
# `--config build.rustflags=[]`, so no RUSTFLAGS-like variable or user config can change float
# results (-C target-cpu=native does) between the two builds. Builds use --locked. The report
# records the names of every environment variable that was dropped and the values of the flag
# variables among them, the toolchain (`rustc -vV`), the sha256 of the overlay patch and of every
# file it patched, and the sha256 of both digests, both rows files (before they are deleted) and
# both compared binaries. A commit that predates --digest/--rows gets the frozen overlay only if
# its files are exactly the ones the patch was made against (replay_overlay_apply.sh); anything
# else is refused.
set -euo pipefail

usage() {
    echo "usage: replay_compare.sh <commitA> <commitB> <clip.oaf> <outdir> [scratchdir] [--expect <file>]" >&2
    exit 2
}

die() {
    echo "replay_compare: $*" >&2
    exit 2
}

expect=""
pos=()
while [ $# -gt 0 ]; do
    case "$1" in
    --expect)
        [ $# -ge 2 ] || usage
        expect="$2"
        shift 2
        ;;
    -*) usage ;;
    *)
        pos+=("$1")
        shift
        ;;
    esac
done
[ "${#pos[@]}" -ge 4 ] && [ "${#pos[@]}" -le 5 ] || usage
spec_a="${pos[0]}"
spec_b="${pos[1]}"
clip="${pos[2]}"
outdir="${pos[3]}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(git -C "$here" rev-parse --show-toplevel)"

# A commit spec is a hash or a ref name: no options, no revision syntax, nothing that names the
# working tree or a dirty state.
check_spec() {
    case "$1" in
    "" | . | .. | -* | *-dirty | *..* | *@\{* | *[[:space:]]* | *[~^:\\]*) die "not a commit hash or ref: '$1'" ;;
    esac
    if ! [[ "$1" =~ ^[A-Za-z0-9._/+-]+$ ]]; then
        die "not a commit hash or ref: '$1'"
    fi
}
resolve() {
    git -C "$repo" rev-parse --verify --quiet "$1^{commit}" || die "no such commit: '$1'"
}
check_spec "$spec_a"
check_spec "$spec_b"
commit_a="$(resolve "$spec_a")"
commit_b="$(resolve "$spec_b")"

[ -f "$clip" ] || die "no such clip: $clip"
[ -z "$expect" ] || [ -f "$expect" ] || die "no such expect file: $expect"
mkdir -p "$outdir"
outdir="$(cd "$outdir" && pwd)"
clip="$(cd "$(dirname "$clip")" && pwd)/$(basename "$clip")"
for f in report.txt summary.txt; do
    [ ! -e "$outdir/$f" ] || die "$outdir/$f exists; use a new output directory"
done
scratch="${pos[4]:-$outdir/work}"
mkdir -p "$scratch"
scratch="$(cd "$scratch" && pwd)"

guard=()
if [ "${REPLAY_COMPARE_GUARD+set}" = set ]; then
    # shellcheck disable=SC2206
    guard=(${REPLAY_COMPARE_GUARD})
else
    guard=(systemd-run --user --scope -q -p "MemoryMax=${GUARD_MEM:-6G}")
fi
toolchain="${REPLAY_COMPARE_TOOLCHAIN:-1.89.0}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

clip_sha="$(sha256sum "$clip" | cut -d' ' -f1)"
# What the caller's environment held, recorded before it is cleared for the builds.
flags_before="RUSTFLAGS=[${RUSTFLAGS-}] CARGO_ENCODED_RUSTFLAGS=[${CARGO_ENCODED_RUSTFLAGS-}] CARGO_BUILD_RUSTFLAGS=[${CARGO_BUILD_RUSTFLAGS-}] RUSTDOCFLAGS=[${RUSTDOCFLAGS-}]"
kept_vars=" PATH HOME RUSTUP_HOME CARGO_HOME TMPDIR "
dropped_vars="$(env | cut -d= -f1 | sort | while read -r n; do case "$kept_vars" in *" $n "*) ;; *) echo "$n" ;; esac; done | tr '\n' ' ')"
# The environment cargo and rustc run in: nothing but these.
clean_env_prefix=(env -i "PATH=$PATH" "HOME=${HOME-}")
[ -z "${RUSTUP_HOME+x}" ] || clean_env_prefix+=("RUSTUP_HOME=$RUSTUP_HOME")
[ -z "${CARGO_HOME+x}" ] || clean_env_prefix+=("CARGO_HOME=$CARGO_HOME")
[ -z "${TMPDIR+x}" ] || clean_env_prefix+=("TMPDIR=$TMPDIR")
rustc_vv="$("${clean_env_prefix[@]}" rustc "+$toolchain" -vV 2>&1)" || die "no rustc for toolchain $toolchain"
sha() { sha256sum "$1" | cut -d' ' -f1; }
worktrees=()
cleanup() {
    for w in "${worktrees[@]}"; do
        git -C "$repo" worktree remove --force "$w" >/dev/null 2>&1 || true
    done
    git -C "$repo" worktree prune >/dev/null 2>&1 || true
    rm -rf "$scratch/target-A" "$scratch/target-B" "$scratch/run-A" "$scratch/run-B"
    rmdir "$scratch" 2>/dev/null || true
}
trap cleanup EXIT

# build_and_run <label> <commit>: leaves digest-<label>.txt, rows-<label>.bin, time-<label>.txt
build_and_run() {
    local label="$1" commit="$2" wt="$scratch/wt-$1" tgt="$scratch/target-$1"
    git -C "$repo" worktree add --detach "$wt" "$commit" >/dev/null
    worktrees+=("$wt")
    # A commit from before the digest flags existed gets the frozen overlay patch, but only
    # if it has neither the canonical module nor the flags; one with exactly one of them is a
    # half-way commit and is refused.
    local has_canon=0 has_flag=0
    git -C "$repo" cat-file -e "$commit:crates/oa-track/src/canon.rs" 2>/dev/null && has_canon=1
    grep -q -- '"--digest"' "$wt/crates/oa-track/src/main.rs" && has_flag=1
    if [ "$has_canon" = 0 ] && [ "$has_flag" = 0 ]; then
        "$here/replay_overlay_apply.sh" "$wt" "$commit" "$here/replay_compare_overlay.patch" \
            "$outdir/overlay-$label.txt" 2>"$outdir/overlay-$label.err" ||
            die "$commit has no --digest/--rows and the overlay patch is refused for it: $(cat "$outdir/overlay-$label.err")"
        echo "$label: overlay applied (commit predates --digest/--rows)" >&2
    elif [ "$has_canon" != "$has_flag" ]; then
        die "$commit has the canonical module or the --digest flag but not both (half-way commit); refusing"
    fi
    echo "$label: building $commit" >&2
    (
        cd "$wt"
        "${guard[@]}" "${clean_env_prefix[@]}" CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" \
            CARGO_TARGET_DIR="$tgt" cargo "+$toolchain" build --release --locked \
            --config 'build.rustflags=[]' -p oa-track --bin oa-track-run >"$outdir/build-$label.log" 2>&1
    ) || die "build of $commit failed (see $outdir/build-$label.log)"
    echo "$label: running" >&2
    mkdir -p "$scratch/run-$label"
    local timer=()
    if [ -x /usr/bin/time ]; then
        timer=(/usr/bin/time -v -o "$outdir/time-$label.txt")
    fi
    "${guard[@]}" "${timer[@]}" "$tgt/release/oa-track-run" --adsb "$clip" \
        --out "$scratch/run-$label" --no-score \
        --digest "$outdir/digest-$label.txt" --rows "$outdir/rows-$label.bin" \
        >"$outdir/run-$label.log" 2>&1 || die "run of $commit failed (see $outdir/run-$label.log)"
    # What was compared, hashed before anything is deleted.
    {
        echo "digest sha256: $(sha "$outdir/digest-$label.txt")"
        echo "rows sha256: $(sha "$outdir/rows-$label.bin")"
        echo "binary sha256: $(sha "$tgt/release/oa-track-run")"
    } >"$outdir/hashes-$label.txt"
    git -C "$repo" worktree remove --force "$wt" >/dev/null
    rm -rf "$tgt" "$scratch/run-$label"
}

build_and_run A "$commit_a"
build_and_run B "$commit_b"

# oa-replay-diff comes from the current checkout, not from either commit under test.
echo "building oa-replay-diff" >&2
diff_tgt="${CARGO_TARGET_DIR:-$repo/target}"
(
    cd "$repo"
    "${guard[@]}" "${clean_env_prefix[@]}" CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" \
        CARGO_TARGET_DIR="$diff_tgt" cargo "+$toolchain" build --release --locked \
        --config 'build.rustflags=[]' -p oa-replay-diff >"$outdir/build-diff.log" 2>&1
) || die "build of oa-replay-diff failed (see $outdir/build-diff.log)"
diff_bin="$diff_tgt/release/oa-replay-diff"
[ -x "$diff_bin" ] || die "the built oa-replay-diff is not at $diff_bin"
echo "diff binary sha256: $(sha "$diff_bin")" >"$outdir/hashes-diff.txt"

args=("$outdir/digest-A.txt" "$outdir/digest-B.txt"
    --rows-a "$outdir/rows-A.bin" --rows-b "$outdir/rows-B.bin"
    --summary "$outdir/summary.txt")
[ -z "$expect" ] || args+=(--expect "$expect")
status=0
"$diff_bin" "${args[@]}" >"$outdir/diff.txt" 2>"$outdir/diff.err" || status=$?
{
    echo "replay comparison"
    echo "commit A: $commit_a ($spec_a)"
    echo "commit B: $commit_b ($spec_b)"
    echo "clip: $clip"
    echo "clip sha256: $clip_sha"
    echo "builds ran under env -i with only PATH, HOME, RUSTUP_HOME, CARGO_HOME, TMPDIR, CARGO_BUILD_JOBS, CARGO_TARGET_DIR, and --config build.rustflags=[]"
    echo "environment flags cleared for the builds: $flags_before"
    echo "environment variables dropped: $dropped_vars"
    echo "toolchain: $toolchain"
    echo "$rustc_vv"
    for l in A B; do
        if [ -f "$outdir/overlay-$l.txt" ]; then
            echo "overlay applied to commit $l:"
            sed 's/^/  /' "$outdir/overlay-$l.txt"
        else
            echo "overlay for commit $l: none (the commit has --digest/--rows)"
        fi
    done
    for l in A B; do
        echo "run $l inputs and outputs:"
        sed 's/^/  /' "$outdir/hashes-$l.txt"
    done
    echo "diff tool:"
    sed 's/^/  /' "$outdir/hashes-diff.txt"
    [ -z "$expect" ] || echo "expect file: $expect"
    echo "oa-replay-diff exit status: $status"
    echo
    cat "$outdir/diff.txt"
    cat "$outdir/diff.err"
} >"$outdir/report.txt"
{
    echo "commit_a=$commit_a"
    echo "commit_b=$commit_b"
    echo "clip_sha256=$clip_sha"
    echo "rustflags_cleared=yes"
    echo "env_dropped=$dropped_vars"
    for l in A B; do
        sed "s/^\\([a-z ]*\\) sha256: \\(.*\\)$/\\1_$l=\\2/; s/ /_/g" "$outdir/hashes-$l.txt"
    done
    echo "rustflags_before=$flags_before"
    echo "toolchain=$toolchain"
    for l in A B; do
        if [ -f "$outdir/overlay-$l.txt" ]; then echo "overlay_$l=applied"; else echo "overlay_$l=none"; fi
    done
    echo "status=$status"
} >>"$outdir/summary.txt"
if [ "${REPLAY_COMPARE_KEEP_ROWS:-0}" != 1 ]; then
    rm -f "$outdir/rows-A.bin" "$outdir/rows-B.bin"
fi
cat "$outdir/report.txt"
exit "$status"
