#!/usr/bin/env python3
"""Refuse service start unless measured 30-day journal growth fits with reserve."""
import os
from pathlib import Path
import shutil
import stat
import sys

FEEDS = ("SFDPS", "STDDS", "TBFM")
HOURS_30_DAYS = 30 * 24
SAFETY_PERCENT = 200
root = Path(os.environ.get("OA_JOURNAL_ROOT", str(Path.home() / ".local/share/openatca-observer/journal")))
state_root = Path(os.environ.get("OA_STATE_ROOT", str(Path.home() / ".local/share/openatca-observer/state")))
target_feed = sys.argv[1] if len(sys.argv) == 2 else ""
if target_feed not in FEEDS:
    raise SystemExit("supply one supported feed name")
def refuse(message):
    print(message, file=sys.stderr)
    raise SystemExit(75)

if (state_root / f"{target_feed}.recording-failed").exists():
    refuse(f"{target_feed} collector is latched after recording failure; inspect before clearing the latch")
cap = int(os.environ.get("OA_JOURNAL_MAX_BYTES", "2147483648"))
reserve = int(os.environ.get("OA_DISK_RESERVE_BYTES", str(512 * 1024 * 1024)))
if cap <= 0 or reserve < 0:
    refuse("invalid journal cap or disk reserve")
rates = {}
required = {}
used_by_feed = {}
for feed in FEEDS:
    name = f"OA_EXPECTED_BYTES_PER_HOUR_{feed}"
    try:
        rate = int(os.environ.get(name, "0"))
    except ValueError:
        refuse(f"{name} must be a positive measured integer")
    if rate <= 0:
        refuse(f"{name} must be a positive measured integer; refusing queue consumption")
    rates[feed] = rate
    required[feed] = (rate * HOURS_30_DAYS * SAFETY_PERCENT + 99) // 100

for feed in FEEDS:
    directory = root / feed
    if not directory.exists():
        continue
    if directory.is_symlink() or not directory.is_dir():
        refuse(f"refusing unsafe journal directory: {directory}")
    entries = list(os.scandir(directory))
    if len(entries) > 16384:
        refuse(f"journal entry cap exceeded for {feed}")
    for entry in entries:
        if entry.name.endswith((".oaf", ".oaf.gz")):
            info = entry.stat(follow_symlinks=False)
            if not stat.S_ISREG(info.st_mode):
                refuse(f"refusing non-regular journal entry: {entry.name}")
            used_by_feed[feed] = used_by_feed.get(feed, 0) + info.st_size
feed_used = used_by_feed.get(target_feed, 0)
if feed_used >= cap:
    refuse(f"{target_feed} journal reached its configured byte cap; refusing to consume its queue")
target_runway = cap - feed_used
if target_runway < required[target_feed]:
    refuse(f"{target_feed} remaining journal cap cannot hold its measured 30-day budget: "
           f"remaining={target_runway} required={required[target_feed]} rate_bytes_per_hour={rates[target_feed]} "
           f"safety_percent={SAFETY_PERCENT}")
future_growth = sum(required.values())
space_path = root if root.exists() else root.parent if root.parent.exists() else Path.home()
free = shutil.disk_usage(space_path).free
if free < future_growth + reserve:
    refuse(f"insufficient free disk for measured 30-day journal growth: free={free} "
           f"required_growth={future_growth} reserve={reserve}")
print(f"journal_space_check=OK feed={target_feed} measured_bytes_per_hour={rates[target_feed]} "
      f"hours={HOURS_30_DAYS} safety_percent={SAFETY_PERCENT} "
      f"feed_30day_budget={required[target_feed]} remaining_cap={target_runway} "
      f"all_feed_30day_growth={future_growth} free={free} reserve={reserve}")
