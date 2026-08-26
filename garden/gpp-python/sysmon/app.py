#!/usr/bin/env python3
"""sysmon — a live process/CPU/memory monitor as a Python GPP app.

The drawer (sysmon.ptl) asks `query("procs", "<field>:<dir>")` — the sort
spec comes from its table's click-to-sort header — and this provider answers
with a freshly sampled, already-sorted process table from `ps aux`, plus the
load averages. A short max-age + stale-while-revalidate policy keeps the
numbers live without spinner flicker: the host re-asks about once a second
and serves the previous sample while the refresh runs.

Launch (see garden/docs/writing-gpp-apps-python.md):

    garden --subprocess python3 garden/gpp-python/sysmon/app.py

Pass `--dev` to hot-reload sysmon.ptl on save. Stdlib only; no pip installs.
"""

import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from gpp import CachePolicy, PanelUi, Provider, Reply, serve  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
DRAWER = os.path.join(HERE, "sysmon.ptl")

# Sort fields, in the drawer's column order. The wire arg is "<field>:<dir>".
SORT_FIELDS = {"pid", "user", "cpu", "mem", "rss", "command"}
MAX_ROWS = 250  # plenty for a pane; keeps /state and the wire small


def parse_sort(spec):
    field, _, direction = (spec or "").partition(":")
    if field not in SORT_FIELDS:
        field = "cpu"
        direction = "desc"
    return field, direction != "asc"


def sample(spec):
    """One `ps aux` sample, sorted per `spec`, shaped for the drawer."""
    ps = subprocess.run(["ps", "aux"], capture_output=True, text=True)
    if ps.returncode != 0:
        return Reply.error(f"ps aux failed: {ps.stderr.strip() or ps.returncode}")

    procs = []
    for line in ps.stdout.splitlines()[1:]:
        # USER PID %CPU %MEM VSZ RSS TT STAT STARTED TIME COMMAND
        parts = line.split(None, 10)
        if len(parts) < 11:
            continue
        try:
            procs.append({
                "user": parts[0],
                "pid": int(parts[1]),
                "cpu": float(parts[2]),
                "mem": float(parts[3]),
                "rss": int(parts[5]),  # KB
                "command": parts[10],
            })
        except ValueError:
            continue  # a header echo or a malformed row

    field, reverse = parse_sort(spec)
    procs.sort(key=lambda p: p[field], reverse=reverse)

    rows = [
        [
            p["pid"],
            p["user"],
            f"{p['cpu']:.1f}",
            f"{p['mem']:.1f}",
            human_kb(p["rss"]),
            p["command"][:160],
        ]
        for p in procs[:MAX_ROWS]
    ]

    try:
        load1, load5, load15 = os.getloadavg()
    except (OSError, AttributeError):
        load1 = load5 = load15 = 0.0

    data = {
        "procs": rows,
        "proc_count": len(procs),
        "cpu_total": round(sum(p["cpu"] for p in procs), 1),
        "ncpu": os.cpu_count() or 1,
        "load1": round(load1, 2),
        "load5": round(load5, 2),
        "load15": round(load15, 2),
        "rss_total_gb": round(sum(p["rss"] for p in procs) / (1024 * 1024), 2),
    }
    return Reply.json(data).cache(CachePolicy.max_age(1.0).stale_while_revalidate(10.0))


def human_kb(kb):
    if kb >= 1024 * 1024:
        return f"{kb / (1024 * 1024):.1f}G"
    if kb >= 1024:
        return f"{kb / 1024:.0f}M"
    return f"{kb}K"


def main():
    provider = Provider().query("procs", lambda state, ctx: sample(ctx.arg_str()))
    ui = PanelUi.from_file("sysmon", DRAWER)
    serve(provider, ui, watch="--dev" in sys.argv)


if __name__ == "__main__":
    main()
