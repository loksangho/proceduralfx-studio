#!/usr/bin/env python3
"""How many times ProceduralFX Studio and bevy_procedural_fx have been downloaded.

    python3 scripts/download_stats.py

Prints three measures for the app: every download GitHub counted; an estimate of other
people's downloads, without your own (see OWN below: edit it if it's wrong); and a floor,
which also takes off one download per file per release, as bots that fetch every new
release file (link checkers, mirrors, archives) would make. Then
the Bevy crate's downloads from crates.io, and what changed since the last run (kept in
~/.proceduralfx-download-stats.json).

Counts are downloads, not people: one person downloading twice counts twice, and the
in-app updater's downloads count too (the Mac .zip, the Windows .zip and the Flatpak are
what it fetches; the DMG is only ever downloaded by hand). crates.io also counts
automated downloads (mirrors, other people's CI). Uses only public data: no sign-in.
"""

import json
import pathlib
import re
import sys
import urllib.request
from datetime import datetime, timezone

REPO = "loksangho/proceduralfx-studio"
CRATE = "bevy_procedural_fx"
HISTORY = pathlib.Path.home() / ".proceduralfx-download-stats.json"

# Your own downloads, per release, as the estimate assumes them. Each is
# (kind of file, how many of yours per release, first release it applies to).
OWN = [
    # Your Mac's in-app updater fetches each release's Mac .zip.
    ("mac-zip", 1, "0.1.0"),
    # You test each Windows release on your Windows PC (Windows builds began at 0.12.0).
    ("windows", 1, "0.12.0"),
    # No Linux machine of yours, and you build the DMG yourself: none of those.
]


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": "proceduralfx-download-stats", "Accept": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def version_key(text):
    return tuple(int(n) for n in re.findall(r"\d+", text)[:3])


def kind(name):
    name = name.lower()
    if name.endswith(".dmg"):
        return "dmg"
    if name.endswith(".flatpak"):
        return "flatpak"
    if "windows" in name and name.endswith(".zip"):
        return "windows"
    if name.endswith(".zip"):
        return "mac-zip"
    return "other"


LABELS = {"windows": "Windows", "dmg": "Mac DMG", "mac-zip": "Mac .zip", "flatpak": "Linux Flatpak", "other": "other"}


def releases():
    out = []
    page = 1
    while True:
        batch = fetch(f"https://api.github.com/repos/{REPO}/releases?per_page=100&page={page}")
        if not batch:
            return out
        for release in batch:
            tag = release["tag_name"]
            # CI's prebuilt Windows libraries, not the app.
            if not tag.startswith("v"):
                continue
            counts = {}
            for asset in release["assets"]:
                k = kind(asset["name"])
                counts[k] = counts.get(k, 0) + asset["download_count"]
            out.append({"version": tag[1:], "date": (release.get("published_at") or "")[:10], "counts": counts})
        page += 1


def own(version, counts):
    """Your own downloads of a release, as OWN assumes, never more than were counted."""
    mine = {}
    for k, n, since in OWN:
        if version_key(version) >= version_key(since) and counts.get(k, 0) > 0:
            mine[k] = min(n, counts[k])
    return mine


def main():
    try:
        app = sorted(releases(), key=lambda r: version_key(r["version"]))
        crate = fetch(f"https://crates.io/api/v1/crates/{CRATE}")
    except Exception as error:  # noqa: BLE001 - a network problem: say so plainly
        sys.exit(f"Couldn't fetch the numbers: {error}")

    kinds = ["windows", "dmg", "mac-zip", "flatpak"]
    print(f"ProceduralFX Studio downloads, {datetime.now(timezone.utc):%Y-%m-%d %H:%M} UTC\n")
    print(f"{'release':<9} {'date':<10}  " + "  ".join(f"{LABELS[k]:>13}" for k in kinds) + f"  {'all':>5}  {'others':>6}  {'floor':>5}")
    totals, others_total, floor_total = {k: 0 for k in kinds}, 0, 0
    for release in app:
        counts = release["counts"]
        mine = own(release["version"], counts)
        everyone = sum(counts.get(k, 0) for k in kinds)
        others = everyone - sum(mine.values())
        others_total += others
        # One bot download of each file left after yours.
        floor = sum(max(counts.get(k, 0) - mine.get(k, 0) - 1, 0) for k in kinds)
        floor_total += floor
        for k in kinds:
            totals[k] += counts.get(k, 0)
        print(f"{release['version']:<9} {release['date']:<10}  " + "  ".join(f"{counts.get(k, 0):>13}" for k in kinds) + f"  {everyone:>5}  {others:>6}  {floor:>5}")
    everyone_total = sum(totals.values())
    print(f"{'total':<20} " + "  ".join(f"{totals[k]:>13}" for k in kinds) + f"  {everyone_total:>5}  {others_total:>6}  {floor_total:>5}")
    print("\n  all:    every download GitHub counted")
    print("  others: without your own, as estimated (OWN at the top of this script)")
    print("  floor:  others, less one bot download per file per release")
    # Fresh installs: the DMG and first-time Windows/Linux downloads can't be told apart
    # from updates, except on the Mac, where only people installing by hand get the DMG.
    print(f"\n  Mac installs by hand (the DMG): {totals['dmg']}")

    c = crate["crate"]
    print(f"\n{CRATE} (crates.io): {c['downloads']} downloads, {c.get('recent_downloads', 0)} in the last 90 days")
    for v in sorted(crate["versions"], key=lambda v: version_key(v["num"])):
        print(f"  {v['num']:<7} {v['created_at'][:10]}  {v['downloads']:>5}")
    print("  (includes automated downloads: mirrors and other people's CI)")

    now = {"when": datetime.now(timezone.utc).isoformat(timespec="minutes"), "all": everyone_total, "others": others_total, "floor": floor_total, "crate": c["downloads"]}
    try:
        before = json.loads(HISTORY.read_text())
    except (OSError, ValueError):
        before = []
    if before:
        last = before[-1]
        print(f"\nSince the last run ({last['when'][:16].replace('T', ' ')} UTC): "
              f"app +{now['all'] - last['all']} (others +{now['others'] - last['others']}, floor +{now['floor'] - last.get('floor', now['floor'])}), crate +{now['crate'] - last['crate']}")
    try:
        HISTORY.write_text(json.dumps((before + [now])[-200:], indent=1))
    except OSError:
        pass


if __name__ == "__main__":
    main()
