#!/usr/bin/env bash
# Writes the MANIFEST of a ZAU reproduction package (read by tools/reproduce_zau.sh).
#
#   tools/zau_manifest.sh DAY COMMIT PKG_DIR EXPORT_NAME CLIP_FILE "CLIP_ARGS" RUN_OUT_DIR "LABEL" PART...
#
#   DAY          YYYY-MM-DD
#   COMMIT       the 40-hex commit the scorer and clip tool are built from (git rev-parse HEAD)
#   PKG_DIR      the package directory: holds EXPORT_NAME (and EXPORT_NAME.sha256, and EXPORT_NAME.xz if
#                it is published packed); MANIFEST goes here
#   CLIP_FILE    the ZAU clip made from the PARTs (hashed here, not published)
#   CLIP_ARGS    the clip tool's options besides --out, e.g. "--frame-ms 1000"
#   RUN_OUT_DIR  the scorer's output directory of the reference run (p002.txt, samples.csv)
#   LABEL        the --label the reference run used
#   PART...      the adsb.lol archive parts (paths of the files the clip was made from, in order)
set -euo pipefail
[ "$#" -ge 9 ] || { echo "usage: tools/zau_manifest.sh DAY COMMIT PKG_DIR EXPORT_NAME CLIP_FILE CLIP_ARGS RUN_OUT_DIR LABEL PART..." >&2; exit 2; }
day="$1"; commit="$2"; pkg="$3"; export_name="$4"; clip="$5"; clip_args="$6"; run_out="$7"; label="$8"; shift 8
[ "$#" -le 16 ] || { echo "zau_manifest: at most 16 archive parts are accepted" >&2; exit 2; }
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
package_io() { python3 -IB "$here/repro_package_io.py" "$@"; }
package_io validate-writer-fields "$label" "$clip_args" "$export_name" "$clip" "$@" || exit 2
python3 -I "$here/sealed_guard.py" "$pkg" "$clip" "$run_out" "$@" || exit 2
case "${day//-/}" in 20261120 | 20261121 | 20261122) echo "zau_manifest: $day is a sealed hold-out day" >&2; exit 2 ;; esac
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || { echo "zau_manifest: COMMIT must be 40 lower-case hex digits" >&2; exit 2; }
measure() { package_io measure "$1"; }
sum() { local result n h; result="$(measure "$1")" || return $?; read -r n h <<<"$result"; echo "$h"; }
masked_sum() { package_io masked-hash "$1"; }
tmp="$pkg/MANIFEST.partial"
for p in "$pkg/$export_name" "$pkg/$export_name.sha256" "$pkg/$export_name.xz" \
         "$pkg/MANIFEST" "$tmp" "$run_out/p002.txt" "$run_out/samples.csv"; do
    python3 -I "$here/sealed_guard.py" "$p" || exit 2
done
[ -f "$pkg/$export_name" ] || { echo "zau_manifest: export is missing: $pkg/$export_name" >&2; exit 2; }
[ -f "$pkg/$export_name.sha256" ] || { echo "zau_manifest: export sidecar is missing: $pkg/$export_name.sha256" >&2; exit 2; }
[ -f "$run_out/p002.txt" ] && [ -f "$run_out/samples.csv" ] || { echo "zau_manifest: scorer outputs are missing" >&2; exit 2; }
[ ! -L "$pkg/MANIFEST" ] || { echo "zau_manifest: refusing a symlink MANIFEST" >&2; exit 2; }
[ ! -e "$pkg/MANIFEST" ] || [ -f "$pkg/MANIFEST" ] || { echo "zau_manifest: MANIFEST is not a regular file" >&2; exit 2; }
[ ! -e "$tmp" ] && [ ! -L "$tmp" ] || { echo "zau_manifest: temporary MANIFEST path already exists" >&2; exit 2; }
set -o noclobber

# Read and hash every package input/output through a verified descriptor before writing a manifest.
parts_meta=()
for p in "$@"; do
    result="$(measure "$p")" || exit 2
    read -r n h <<<"$result"
    name="$(basename "$p")"
    parts_meta+=("$name $n $h")
done
clip_info="$(measure "$clip")" || exit 2
read -r clip_size clip_sha <<<"$clip_info"
export_info="$(measure "$pkg/$export_name")" || exit 2
read -r export_size export_sha <<<"$export_info"
if [ -f "$pkg/$export_name.xz" ]; then
    xz_info="$(measure "$pkg/$export_name.xz")" || exit 2
    read -r xz_size xz_sha <<<"$xz_info"
fi
p002_sha="$(sum "$run_out/p002.txt")" || exit 2
p002_masked_sha="$(masked_sum "$run_out/p002.txt")" || exit 2
samples_sha="$(sum "$run_out/samples.csv")" || exit 2
{
    echo "reproduce-zau-manifest 1"
    echo "day $day"
    echo "commit $commit"
    echo "label $label"
    tag="$(basename "$1")"; tag="${tag%.tar.*}"
    echo "adsb-release https://github.com/adsblol/globe_history_2026/releases/tag/$tag"
    for entry in "${parts_meta[@]}"; do
        read -r n bytes digest <<<"$entry"
        echo "adsb-file $n $bytes $digest https://github.com/adsblol/globe_history_2026/releases/download/$tag/$n"
    done
    echo "clip-args $clip_args"
    echo "clip $(basename "$clip") $clip_size $clip_sha"
    echo "eram-export $export_name $export_size $export_sha"
    [ -z "${xz_size:-}" ] || echo "eram-export-xz $export_name.xz $xz_size $xz_sha"
    echo "output p002.txt $p002_sha $p002_masked_sha"
    echo "output samples.csv $samples_sha"
} >"$tmp"
python3 -I "$here/repro_package_io.py" validate-manifest "$tmp" || { rm -f -- "$tmp"; exit 2; }
python3 -I "$here/sealed_guard.py" "$pkg/MANIFEST" "$tmp" || exit 2
[ ! -L "$pkg/MANIFEST" ] && [ ! -L "$tmp" ] || { echo "zau_manifest: refusing a symlink during publication" >&2; exit 2; }
mv -fT -- "$tmp" "$pkg/MANIFEST"
[ -f "$pkg/MANIFEST" ] && [ ! -L "$pkg/MANIFEST" ] || { echo "zau_manifest: MANIFEST publication failed" >&2; exit 2; }
echo "wrote $pkg/MANIFEST"
