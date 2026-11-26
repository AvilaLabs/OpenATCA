#!/bin/sh
set -eu

feed=${1-}
case "$feed" in SFDPS|STDDS|TBFM) ;; *) echo "unsupported feed name" >&2; exit 2 ;; esac
: "${OA_FEED_BIN:=/usr/local/bin/oa-feed}"
: "${OA_BRIDGE_JAR:=/usr/local/lib/openatca/scds-bridge.jar}"
: "${OA_SOURCE_CONFIG_DIR:=$HOME/.config/openatca/sources}"
: "${OA_JOURNAL_ROOT:=$HOME/.local/share/openatca-observer/journal}"
: "${OA_JOURNAL_MAX_BYTES:=2147483648}"
: "${OA_ROTATE_BYTES:=268435456}"
config="$OA_SOURCE_CONFIG_DIR/$feed.env"
journal="$OA_JOURNAL_ROOT/$feed"
if [ ! -x "$OA_FEED_BIN" ]; then echo "oa-feed binary unavailable" >&2; exit 1; fi
if [ ! -r "$OA_BRIDGE_JAR" ]; then echo "SCDS bridge jar unavailable" >&2; exit 1; fi
if [ ! -f "$config" ] || [ ! -r "$config" ]; then echo "external $feed credential/config file unavailable: $config" >&2; exit 1; fi
mkdir -p -m 700 "$journal"
set +e
"$OA_FEED_BIN" --env "$config" --jar "$OA_BRIDGE_JAR" --feed "$feed" \
  --record-raw "$journal" --sync-closed-segments --stop-on-recording-failure \
  --max-bytes "$OA_JOURNAL_MAX_BYTES" --rotate-bytes "$OA_ROTATE_BYTES" \
  --summary-every 10
status=$?
set -e
if [ "$status" -eq 75 ]; then
  state="${HOME}/.local/share/openatca-observer/state"
  mkdir -p -m 700 "$state"
  (umask 077; printf 'recording_failure_utc=%s\nfeed=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$feed" > "$state/$feed.recording-failed")
fi
exit "$status"
