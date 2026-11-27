#!/bin/sh
set -eu
base=${HOME:?}/.local/share/openatca-observer
script=${HOME}/.local/libexec/openatca-observer/oaf_observer.py
set -- /usr/bin/python3 "$script" \
  --journal-root "$base/journal" \
  --state-root "$base/state" \
  --report "$base/reports/observer.json" \
  --capture-manifest "$base/archive/capture-manifest.json" \
  --capture-pin "$base/archive/retention-pin.json" \
  --duration-seconds 2592000 --gap-seconds 60 --poll-seconds 2
if [ -n "${OA_CAPTURE_FROM_UTC-}" ] || [ -n "${OA_CAPTURE_TO_UTC-}" ]; then
  if [ -z "${OA_CAPTURE_FROM_UTC-}" ] || [ -z "${OA_CAPTURE_TO_UTC-}" ]; then
    echo "set both OA_CAPTURE_FROM_UTC and OA_CAPTURE_TO_UTC in the systemd user drop-in" >&2
    exit 2
  fi
  set -- "$@" --capture-from-utc "$OA_CAPTURE_FROM_UTC" --capture-to-utc "$OA_CAPTURE_TO_UTC"
fi
exec "$@"
