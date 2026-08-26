#!/usr/bin/env python3
"""repo-stats — a git repository dashboard as a Python GPP app.

The drawer (repo_stats.ptl) asks `query("stats", "")` once; this provider
shells `git` in the launch directory (the first arg, else the pane's cwd)
and answers with everything the dashboard shows: commits-per-week buckets,
the top authors, and the most recent commits. A directory that is not a git
repository is a clean APP error the drawer surfaces in place.

Launch (see garden/docs/writing-gpp-apps-python.md):

    garden --subprocess python3 garden/gpp-python/repo-stats/app.py [repo-dir]

Pass `--dev` to hot-reload repo_stats.ptl on save. Stdlib only.
"""

import os
import subprocess
import sys
import time
from collections import Counter
from datetime import date, datetime, timedelta

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from gpp import AppError, CachePolicy, PanelUi, Provider, Reply, script_args, serve  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
DRAWER = os.path.join(HERE, "repo_stats.ptl")

WEEKS = 26  # half a year of weekly bars
TOP_AUTHORS = 8
RECENT = 12
LOG_CAP = 5000  # commits parsed for the buckets/authors (totals stay exact)


def git(repo, *args):
    r = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True)
    if r.returncode != 0:
        raise AppError(r.stderr.strip().splitlines()[0] if r.stderr.strip() else f"git {' '.join(args)} failed")
    return r.stdout


def relative(ts, now):
    s = max(0, int(now - ts))
    if s < 3600:
        return f"{s // 60}m ago"
    if s < 86400:
        return f"{s // 3600}h ago"
    if s < 86400 * 30:
        return f"{s // 86400}d ago"
    return datetime.fromtimestamp(ts).strftime("%Y-%m-%d")


def stats(repo):
    try:
        top = git(repo, "rev-parse", "--show-toplevel").strip()
    except AppError:
        raise AppError(f"not a git repo: {repo}")
    branch = git(repo, "rev-parse", "--abbrev-ref", "HEAD").strip()

    # One log pass covers the buckets, the authors, and the recent list.
    try:
        total = int(git(repo, "rev-list", "--count", "HEAD").strip())
    except AppError:
        total = 0  # a repo with no commits yet
    log = ""
    if total > 0:
        log = git(repo, "log", f"-n{LOG_CAP}", "--no-show-signature",
                  "--pretty=format:%h%x09%an%x09%at%x09%s")

    commits = []  # newest first
    for line in log.splitlines():
        parts = line.split("\t", 3)
        if len(parts) < 4:
            continue
        short, author, ts, subject = parts
        try:
            commits.append({"short": short, "author": author, "ts": int(ts), "subject": subject})
        except ValueError:
            continue

    # Weekly buckets: the last WEEKS ISO weeks, oldest → newest, keyed by the
    # Monday that starts each week.
    today = date.today()
    this_monday = today - timedelta(days=today.weekday())
    starts = [this_monday - timedelta(weeks=WEEKS - 1 - i) for i in range(WEEKS)]
    counts = [0] * WEEKS
    first_start = starts[0]
    for c in commits:
        d = date.fromtimestamp(c["ts"])
        monday = d - timedelta(days=d.weekday())
        idx = (monday - first_start).days // 7
        if 0 <= idx < WEEKS:
            counts[idx] += 1
    weeks = [{"label": s.strftime("%m/%d"), "count": n} for s, n in zip(starts, counts)]

    tally = Counter(c["author"] for c in commits)
    authors = [
        {"name": name, "count": n, "frac": round(n / len(commits), 3) if commits else 0.0}
        for name, n in tally.most_common(TOP_AUTHORS)
    ]

    now = time.time()
    recent = [
        {"short": c["short"], "subject": c["subject"], "author": c["author"],
         "when": relative(c["ts"], now)}
        for c in commits[:RECENT]
    ]

    data = {
        "repo": os.path.basename(top) or top,
        "branch": branch,
        "total": total,
        "sampled": len(commits),
        "weeks": weeks,
        "max_week": max(counts) if counts else 0,
        "authors": authors,
        "recent": recent,
    }
    return Reply.json(data).cache(CachePolicy.max_age(5.0).stale_while_revalidate(60.0))


def pick_repo(init):
    # The args mirror the argv (`python3 app.py [repo] [--dev]`), so strip
    # this script's own path and the flags before reading the repo dir.
    args = [a for a in script_args(init) if not a.startswith("-")]
    return args[0] if args else init.cwd


def main():
    provider = Provider(pick_repo).query(
        "stats", lambda repo, ctx: stats(repo))
    ui = PanelUi.from_file("repo-stats", DRAWER,
                           title=lambda repo: f"repo-stats — {os.path.basename(repo.rstrip(os.sep)) or repo}")
    serve(provider, ui, watch="--dev" in sys.argv)


if __name__ == "__main__":
    main()
