#!/usr/bin/env bash
# Reproduces the Chicago Center (ZAU) accuracy comparison against ERAM (scorer P-002) for one
# development day from a published package, and compares every output with the published hash.
#
#   tools/reproduce_zau.sh DAY DATA_DIR OUT_DIR
#
#   DAY       the day of the package, YYYY-MM-DD (it must be the day its MANIFEST names)
#   DATA_DIR  the package: MANIFEST, the ERAM export (plain, or only its .xz, which is unpacked into
#             OUT_DIR) and its .sha256, and the adsb.lol archive parts named in the MANIFEST
#             (download them from the URLs it lists; they are not republished)
#   OUT_DIR   new or empty; the clip, the scorer's output (OUT_DIR/out) and the logs go here
#
# What it does, in order: refuses the sealed hold-out days and any sealed path; checks the size and
# SHA-256 of every input against the MANIFEST; refuses unless this checkout is clean and at the
# commit the MANIFEST names; builds the clip tool and the scorer at that commit; builds the ZAU clip
# from the archive parts; scores it against the ERAM export in a development run; compares the clip
# and both output files with the MANIFEST and prints PASS, or FAIL naming each file that differs.
# p002.txt prints the SHA-256 of the scorer binary, which depends on the machine that built it; it
# is compared with those two values masked (the exact hash is printed too, and also compared).
# Needs bash, git, cargo (the toolchain of rust-toolchain.toml), and python3 with its lzma module.
# Exit: 0 PASS, 1 FAIL, 2 refused (usage, sealed, input, tree).
set -uo pipefail

die() { echo "reproduce_zau: refused: $*" >&2; exit 2; }
[ "$#" -eq 3 ] || { echo "usage: tools/reproduce_zau.sh DAY DATA_DIR OUT_DIR" >&2; exit 2; }
day="$1"; data="$2"; out="$3"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
package_io() { python3 -IB "$here/repro_package_io.py" "$@"; }
repo="$(git -C "$here" rev-parse --show-toplevel)" || die "not inside a git checkout"

# The sealed hold-out days, by digits (every . - _ dropped, as the shared guard reads a name).
[[ "$day" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || die "DAY must be YYYY-MM-DD"
case "${day//-/}" in 20261120 | 20261121 | 20261122) die "$day is a sealed hold-out day" ;; esac
python3 -I "$here/sealed_guard.py" "$data" "$out" || exit 2
[ -d "$data" ] || die "DATA_DIR $data is not a directory"
data="$(cd "$data" && pwd)"
manifest_text="$(package_io read-manifest "$data/MANIFEST")" || exit 2
if [ -e "$out" ]; then
    [ -d "$out" ] && [ -z "$(ls -A "$out")" ] || die "OUT_DIR $out must be new or empty"
fi

# --- the manifest ------------------------------------------------------------------------------
m_day="" m_commit="" m_label="" m_clip_name="" m_clip_size="" m_clip_sha="" m_clip_args=""
m_eram="" m_eram_size="" m_eram_sha="" m_xz="" m_xz_size="" m_xz_sha="" m_csv_sha="" m_txt_sha="" m_txt_norm=""
parts=() part_size=() part_sha=()
first=1
while IFS= read -r line || [ -n "$line" ]; do
    if [ "$first" -eq 1 ]; then
        [ "$line" = "reproduce-zau-manifest 1" ] || die "MANIFEST is not a 'reproduce-zau-manifest 1' file"
        first=0; continue
    fi
    [ -z "$line" ] && continue
    key="${line%% *}"; rest="${line#* }"
    case "$key" in
        '#'*) ;;
        day) m_day="$rest" ;;
        commit) m_commit="$rest" ;;
        label) m_label="$rest" ;;
        adsb-release | adsb-url) ;;
        adsb-file) read -r n s h _ <<<"$rest"; parts+=("$n"); part_size+=("$s"); part_sha+=("$h") ;;
        clip-args) m_clip_args="$rest" ;;
        clip) read -r m_clip_name m_clip_size m_clip_sha <<<"$rest" ;;
        eram-export) read -r m_eram m_eram_size m_eram_sha <<<"$rest" ;;
        eram-export-xz) read -r m_xz m_xz_size m_xz_sha <<<"$rest" ;;
        output)
            read -r n a b <<<"$rest"
            case "$n" in
                p002.txt) m_txt_sha="$a"; m_txt_norm="$b" ;;
                samples.csv) m_csv_sha="$a" ;;
                *) die "MANIFEST names an unknown output $n" ;;
            esac ;;
        *) die "MANIFEST has an unknown line: $key" ;;
    esac
done <<<"$manifest_text"
[ "$m_day" = "$day" ] || die "the MANIFEST is for $m_day, not $day"
[[ "$m_commit" =~ ^[0-9a-f]{40}$ ]] || die "the MANIFEST has no commit"
for v in "$m_clip_name" "$m_clip_sha" "$m_eram" "$m_eram_sha" "$m_csv_sha" "$m_txt_sha" "$m_txt_norm"; do
    [ -n "$v" ] || die "the MANIFEST is incomplete"
done
[ "${#parts[@]}" -gt 0 ] || die "the MANIFEST lists no archive part"
for n in "$m_clip_name" "$m_eram" "$m_xz" "${parts[@]}"; do
    [ -n "$n" ] || continue
    [[ "$n" =~ ^[A-Za-z0-9._+-]+$ ]] || die "the MANIFEST names a file that is not a plain name: $n"
done
[ "${#parts[@]}" -le 16 ] || die "MANIFEST lists more than 16 archive parts"

# --- inputs ------------------------------------------------------------------------------------
check() { # name size sha
    local f="$data/$1"
    if package_io check "$f" "$2" "$3"; then
        :
    else
        local check_status=$?
        [ "$check_status" -eq 3 ] && die "$1 is not in $data"
        exit 2
    fi
    echo "input ok: $1"
}
digest_of() { local result n h; result="$(package_io measure "$1")" || return $?; read -r n h <<<"$result"; echo "$h"; }
eram_plain=0
if package_io present "$data/$m_eram" >/dev/null; then
    check "$m_eram" "$m_eram_size" "$m_eram_sha"; eram_plain=1
else
    present_status=$?
    [ "$present_status" -eq 3 ] || die "could not safely open $m_eram"
    [ -n "$m_xz" ] || die "$m_eram is not in $data"
    check "$m_xz" "$m_xz_size" "$m_xz_sha"
fi
package_io present "$data/$m_eram.sha256" >/dev/null || die "$m_eram.sha256 is missing or refused"
for i in "${!parts[@]}"; do check "${parts[$i]}" "${part_size[$i]}" "${part_sha[$i]}"; done

# --- the tree ----------------------------------------------------------------------------------
head="$(git -C "$repo" rev-parse HEAD)"
[ "$head" = "$m_commit" ] || die "HEAD is $head, the MANIFEST names $m_commit (git checkout $m_commit)"
[ -z "$(git -C "$repo" status --porcelain)" ] || die "the working tree is not clean"

# --- build -------------------------------------------------------------------------------------
mkdir -p "$out" || die "cannot create $out"
out="$(cd "$out" && pwd)"
tgt="${CARGO_TARGET_DIR:-$repo/target}"
echo "building at $head"
( cd "$repo" && "$here/p002_build.sh" ) >"$out/build.log" 2>&1 || { echo "FAIL: p002 build failed, see $out/build.log"; exit 1; }
( cd "$repo" && cargo build --release --locked -p oa-adsb --bin oa-adsb-clip ) >>"$out/build.log" 2>&1 ||
    { echo "FAIL: clip build failed, see $out/build.log"; exit 1; }
p002="$tgt/release/examples/p002"; clip="$tgt/release/oa-adsb-clip"
[ -x "$p002" ] && [ -x "$clip" ] || { echo "FAIL: built binaries not found under $tgt/release"; exit 1; }

# --- the clip ----------------------------------------------------------------------------------
cd "$out" || exit 2
args=(); for n in "${parts[@]}"; do args+=("$data/$n"); done
clip_options=(); read -r -a clip_options <<<"$m_clip_args"
"$clip" --out "$m_clip_name" "${clip_options[@]}" "${args[@]}" >clip.out 2>clip.err || { echo "FAIL: the clip tool failed, see $out/clip.err"; exit 1; }
package_io read clip.err | grep -x 'archive: complete' >/dev/null ||
    { echo "FAIL: the clip tool did not log 'archive: complete', see $out/clip.err"; exit 1; }
fail=0
clip_got="$(digest_of "$m_clip_name")" || { echo "FAIL: cannot verify the generated clip"; exit 1; }
if [ "$clip_got" = "$m_clip_sha" ]; then echo "clip ok: $m_clip_name $clip_got"; else echo "DIFFERS: $m_clip_name $clip_got (published $m_clip_sha)"; fail=1; fi

# --- the scorer: paths are relative so the output names no path of this machine -----------------
if [ "$eram_plain" -eq 1 ]; then
    ln -s "$data/$m_eram" "$m_eram"
else
    package_io unpack-xz "$data/$m_xz" "$m_eram" "$m_eram_size" "$m_eram_sha" || { echo "FAIL: cannot unpack or verify $m_xz"; exit 1; }
    echo "input ok: $m_eram (unpacked from $m_xz)"
fi
ln -s "$data/$m_eram.sha256" "$m_eram.sha256"
: >ledger.txt
"$p002" --development --label "$m_label" --adsb "$m_clip_name" --eram-export "$m_eram" --out out --ledger ledger.txt >p002.stdout 2>p002.err ||
    { echo "FAIL: the scorer failed, see $out/p002.err"; exit 1; }
csv_got="$(digest_of out/samples.csv)" || { echo "FAIL: cannot verify samples.csv"; exit 1; }
txt_got="$(digest_of out/p002.txt)" || { echo "FAIL: cannot verify p002.txt"; exit 1; }
txt_norm="$(package_io masked-hash out/p002.txt)" || { echo "FAIL: cannot normalize p002.txt"; exit 1; }
if [ "$csv_got" = "$m_csv_sha" ]; then echo "output ok: samples.csv $csv_got"; else echo "DIFFERS: samples.csv $csv_got (published $m_csv_sha)"; fail=1; fi
if [ "$txt_norm" = "$m_txt_norm" ]; then
    echo "output ok: p002.txt (binary hash masked) $txt_norm"
    if [ "$txt_got" = "$m_txt_sha" ]; then echo "output ok: p002.txt exactly, $txt_got"; else echo "note: p002.txt differs only in the scorer binary's hash (a different build): $txt_got (published $m_txt_sha)"; fi
else
    echo "DIFFERS: p002.txt (binary hash masked) $txt_norm (published $m_txt_norm)"; fail=1
fi
if [ "$fail" -eq 0 ]; then echo "PASS"; exit 0; fi
echo "FAIL"; exit 1
