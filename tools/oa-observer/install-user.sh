#!/bin/sh
set -eu
umask 077
src=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
lib="$HOME/.local/libexec/openatca-observer"
units="$HOME/.config/systemd/user"
data="$HOME/.local/share/openatca-observer"
config="$HOME/.config/openatca/sources"
budget_config="$HOME/.config/openatca-observer"
install -d -m 700 "$lib" "$units" "$data/journal/SFDPS" "$data/journal/STDDS" "$data/journal/TBFM" \
  "$data/state" "$data/reports" "$data/archive" "$config" "$budget_config"
install -m 755 "$src/collect-one.sh" "$lib/collect-one.sh"
install -m 755 "$src/run-observer.sh" "$lib/run-observer.sh"
install -m 755 "$src/oaf_observer.py" "$lib/oaf_observer.py"
install -m 755 "$src/check-disk.py" "$lib/check-disk.py"
install -m 644 "$src/systemd/user/openatca-collector@.service" "$units/openatca-collector@.service"
install -m 644 "$src/systemd/user/openatca-observer.service" "$units/openatca-observer.service"
install -m 600 "$src/storage-budget.env.example" "$budget_config/storage-budget.env.example"
systemctl --user daemon-reload
cat <<EOF
Installed the local unit files and observer tools. Nothing was enabled or started.

Before starting, install the existing oa-feed binary at:
  $HOME/.local/bin/oa-feed
install the existing SCDS bridge jar at:
  $HOME/.local/lib/openatca/scds-bridge.jar
and place the already-approved SFDPS.env, STDDS.env, and TBFM.env subscription files at:
  $config/
The collector passes each file path to oa-feed; this package never opens or copies their contents.

Before starting collectors, measure each feed's local journal growth rate, copy
  cp $budget_config/storage-budget.env.example $budget_config/storage-budget.env
then edit $budget_config/storage-budget.env and replace all three zero values with measured
positive integer bytes/hour. Preflight
requires each 30-day measured budget at 2x safety to fit its remaining journal cap and
requires free disk for all three projected budgets plus reserve. Missing or insufficient
measurements refuse collector start before subscription consumption.

The observer unit includes the approved acquisition range 2026-11-30T05:00:00Z through
2026-12-01T06:00:00Z. Review these bounds before deployment. Its manifest selects complete
original OAF2 frames by recorder receive time in [from,to); it does not claim source-event latency
or completeness and does not widen the window.

Enable collectors and start the 30-day observer with:
  systemctl --user enable --now openatca-collector@SFDPS.service openatca-collector@STDDS.service openatca-collector@TBFM.service openatca-observer.service
For service operation after logout/reboot, the account needs systemd lingering enabled by the
Debian administrator (loginctl enable-linger USER).
EOF
