#!/usr/bin/env python3
"""Refuse service start unless the bounded three-feed journal budget fits with reserve."""
import os
from pathlib import Path
import shutil
import stat
import sys

FEEDS = ("SFDPS", "STDDS", "TBFM")
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
used = 0
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
            used += info.st_size
feed_dir = root / target_feed
feed_used = 0
if feed_dir.exists():
    for entry in os.scandir(feed_dir):
        if entry.name.endswith((".oaf", ".oaf.gz")):
            feed_used += entry.stat(follow_symlinks=False).st_size
if feed_used >= cap:
    refuse(f"{target_feed} journal reached its configured byte cap; refusing to consume its queue")
budget = cap * len(FEEDS)
remaining = max(0, budget - used)
space_path = root if root.exists() else root.parent if root.parent.exists() else Path.home()
free = shutil.disk_usage(space_path).free
if free < remaining + reserve:
    refuse(f"insufficient free disk for journal cap: free={free} remaining_budget={remaining} reserve={reserve}")
print(f"journal_space_check=OK used={used} bounded_total={budget} free={free} reserve={reserve}")
