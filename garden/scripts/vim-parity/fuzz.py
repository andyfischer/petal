#!/usr/bin/env python3
"""Vim-parity differential fuzzer for the Garden editor.

Generates random keystroke programs drawn from *only the Vim subset Garden
implements*, runs each program against both

  * real Vim (nvim -u NONE, the parity oracle), and
  * a live headless Garden instance (driven over its debug server),

starting from identical buffer + cursor state, then reports every case where
the two disagree on the resulting buffer, cursor, or mode. Each disagreement is
delta-debugged down to a minimal reproducer and clustered by signature.

Divergences are, by construction, candidate Garden bugs: the generator never
emits a key Garden doesn't implement, so "Garden did something different from
Vim" is the whole point.

Usage:
    # Garden must already be running headless with a debug server:
    #   cargo run -p garden-app -- --headless --debug-port 8091 <file>
    python3 fuzz.py --port 8091 --count 500 --seed 1
"""

import argparse
import json
import os
import random
import subprocess
import sys
import tempfile
import urllib.request
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))

# ---------------------------------------------------------------------------
# Token model
#
# A "program" is a flat list of tokens. Each token executes identically in
# concept against both editors; the two backends just translate it differently.
#   ("key", ch)  a literal keypress (letters, digits, symbols, uppercase)
#   ("esc",)     Escape
#   ("enter",)   Enter / <CR>
#   ("bs",)      Backspace
#   ("ctrlr",)   Ctrl-R (redo)
# A terminating Escape is *always* appended at run time (not stored in the
# program) so both editors finish in a comparable Normal-mode state and any
# program prefix is a valid, self-terminating program during minimization.
# ---------------------------------------------------------------------------

VIM_BYTES = {"esc": "\x1b", "enter": "\r", "bs": "\x08", "ctrlr": "\x12"}


def to_vim_bytes(program):
    """Raw byte string for the program, WITHOUT the auto-terminating Escape."""
    out = []
    for t in program:
        if t[0] == "key":
            out.append(t[1])
        else:
            out.append(VIM_BYTES[t[0]])
    return "".join(out)


def pretty(program):
    """Human-readable rendering of a program for reports."""
    names = {"esc": "<Esc>", "enter": "<CR>", "bs": "<BS>", "ctrlr": "<C-R>"}
    parts = []
    for t in program:
        if t[0] == "key":
            parts.append("<Spc>" if t[1] == " " else t[1])
        else:
            parts.append(names[t[0]])
    return "".join(parts)


# ---------------------------------------------------------------------------
# Garden driver (debug server over HTTP)
# ---------------------------------------------------------------------------


class Garden:
    def __init__(self, port):
        self.base = f"http://127.0.0.1:{port}"

    def _post(self, path, obj):
        data = json.dumps(obj).encode()
        req = urllib.request.Request(
            self.base + path, data=data, headers={"Content-Type": "application/json"}
        )
        with urllib.request.urlopen(req, timeout=10) as r:
            return json.loads(r.read())

    def _get(self, path):
        with urllib.request.urlopen(self.base + path, timeout=10) as r:
            return r.read()

    def key(self, k, mods=None):
        body = {"key": k}
        if mods:
            body["mods"] = mods
        return self._post("/key", body)

    def text(self, s):
        return self._post("/text", {"text": s})

    def buffer(self):
        return self._get("/buffer/0").decode("utf-8", "replace")

    def state(self):
        return json.loads(self._get("/state"))

    def send_token(self, t):
        if t[0] == "key":
            ch = t[1]
            if ch == " ":
                self.key("space")
            else:
                self.key(ch)
        elif t[0] == "esc":
            self.key("escape")
        elif t[0] == "enter":
            self.key("enter")
        elif t[0] == "bs":
            self.key("backspace")
        elif t[0] == "ctrlr":
            self.key("r", ["ctrl"])

    def reset(self, lines):
        """Load `lines` into pane 0 and park the cursor at (0,0), Normal mode."""
        # Two escapes guarantee we leave insert / visual / operator-pending from
        # whatever the previous case left behind.
        self.key("escape")
        self.key("escape")
        self.key("a", ["cmd"])  # select all
        self.text("\n".join(lines))
        self.key("escape")
        # gg lands on first non-blank; 0 forces column 0 to match the oracle.
        self.key("g")
        self.key("g")
        self.key("0")

    def run(self, lines, program):
        self.reset(lines)
        for t in program:
            self.send_token(t)
        self.key("escape")  # terminating escape
        st = self.state()["panes"][0]
        buf = self.buffer()
        return {
            "lines": buf.split("\n"),
            "cursor": [st["cursor"]["line"], st["cursor"]["col"]],
            "mode": st["mode"],
        }


# ---------------------------------------------------------------------------
# Vim oracle — one `feedkeys(..., 'ntx')` process per case.
#
# We drive the oracle with vimscript `feedkeys(keys, 'ntx')`: the keys are
# processed **as if typed interactively** ('t'), with no remapping ('n'), and
# the call blocks until the whole typeahead is consumed ('x'). This is the
# faithful model of a user at the keyboard — a beep (e.g. `b` at column 0) does
# NOT discard the following keystrokes, and pending operator/count state carries
# across keys.
#
# Why not `nvim -s scriptfile` (an earlier version of this oracle)? Script
# replay is faithful for motions/operators/paste, but it **over-joins the undo
# history**: it does not break an undo block at Insert-mode `<Esc>` the way real
# typing does, so `A x<Esc> A y<Esc> u` collapses BOTH inserts into one block
# and a single `u` wrongly removes both. `feedkeys('ntx')` reproduces real vim's
# per-insert-session undo blocks. The two oracles were cross-checked and agree
# on 300 core + 300 paste cases; they differ only on the undo tier, where `-s`
# was the one diverging from interactive vim. See `README.md` and
# `oracle_xcheck.py`.
#
# One process per case keeps cases hermetic (no mode/state bleed) and lets the
# batch run in parallel.
# ---------------------------------------------------------------------------


def to_feedkeys(program):
    """A double-quoted vimscript string for `feedkeys` — literal chars verbatim,
    special keys as `\\<Esc>` / `\\<CR>` / `\\<BS>` / `\\<C-R>` notation."""
    special = {"esc": "\\<Esc>", "enter": "\\<CR>", "bs": "\\<BS>", "ctrlr": "\\<C-R>"}
    out = []
    for t in program:
        if t[0] == "key":
            ch = t[1]
            out.append({"\\": "\\\\", '"': '\\"'}.get(ch, ch))
        else:
            out.append(special[t[0]])
    return "".join(out)


def run_vim_case(init, program):
    # A trailing Esc normalizes to Normal mode so the dumped state is comparable
    # and any program prefix is a valid self-terminating program (minimization).
    keys = to_feedkeys(program) + "\\<Esc>"
    with tempfile.TemporaryDirectory() as d:
        content = os.path.join(d, "buf.txt")
        script = os.path.join(d, "s.vim")
        lout = os.path.join(d, "lines")
        cout = os.path.join(d, "cur")
        with open(content, "w") as f:
            f.write("\n".join(init) + "\n")
        # Configure the oracle to *default Vim* semantics for the options that
        # matter here. `-u NONE` alone is too bare: it flips `startofline` OFF,
        # whereas real Vim (and Garden) land gg/G/dd/C on the first non-blank.
        # `whichwrap=` matches Garden's deliberate non-wrapping h/l/Space/BS.
        with open(script, "w") as f:
            f.write(
                "set noswapfile noautoindent nosmartindent noexpandtab "
                "startofline whichwrap=\n"
                f'call feedkeys("{keys}", "ntx")\n'
                f"call writefile(getline(1,'$'), '{lout}')\n"
                f"call writefile([(line('.')-1).' '.(col('.')-1)], '{cout}')\n"
                "qa!\n"
            )
        try:
            subprocess.run(
                ["nvim", "--headless", "-u", "NONE", "-n", "-i", "NONE",
                 "-S", script, content],
                check=True, capture_output=True, timeout=15,
            )
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
            return {"lines": None, "cursor": None, "mode": "n", "ok": False,
                    "err": str(e)}
        if not (os.path.exists(lout) and os.path.exists(cout)):
            return {"lines": None, "cursor": None, "mode": "n", "ok": False,
                    "err": "no output"}
        with open(lout, "r", encoding="utf-8", errors="replace") as f:
            raw = f.read()
        lines = raw.split("\n")
        if lines and lines[-1] == "":
            lines = lines[:-1]  # drop writefile's trailing newline
        with open(cout) as f:
            cl, cc = f.read().split()
    return {"lines": lines, "cursor": [int(cl), int(cc)], "mode": "n", "ok": True}


def run_vim_batch(cases, workers=8):
    """cases: list of {"id","init","moves"}. Returns id->result (parallelized)."""
    def one(c):
        return c["id"], run_vim_case(c["init"], flatten(c["moves"]))
    out = {}
    with ThreadPoolExecutor(max_workers=workers) as ex:
        for cid, r in ex.map(one, cases):
            out[cid] = r
    return out


# ---------------------------------------------------------------------------
# Content + program generators
# ---------------------------------------------------------------------------

WORDS = ["foo", "bar", "baz", "qux", "hello", "world", "a", "xy", "abc",
         "lorem", "ipsum", "the", "cat", "sat", "on", "mat", "x", "ok"]
PUNCTY = ["(foo)", "[bar]", "{baz}", "a(b)c", "x=y;", "f(x);", "a,b,c", "()", "[]"]


def gen_content(rng):
    n = rng.randint(1, 5)
    lines = []
    for _ in range(n):
        roll = rng.random()
        if roll < 0.12:
            lines.append("")  # empty line
            continue
        indent = " " * rng.choice([0, 0, 0, 2, 4])
        wc = rng.randint(1, 4)
        chunks = []
        for _ in range(wc):
            if rng.random() < 0.25:
                chunks.append(rng.choice(PUNCTY))
            else:
                chunks.append(rng.choice(WORDS))
        lines.append(indent + " ".join(chunks))
    return lines


INSERT_CHARS = list("abcdefgABCXYZ012 .,()")


def gen_insert_text(rng, allow_enter):
    n = rng.randint(1, 4)
    toks = []
    for _ in range(n):
        if allow_enter and rng.random() < 0.15:
            toks.append(("enter",))
        else:
            toks.append(("key", rng.choice(INSERT_CHARS)))
    # occasional backspace
    if rng.random() < 0.15:
        toks.append(("bs",))
    return toks


def maybe_count(rng, p=0.35, hi=6):
    if rng.random() < p:
        n = rng.randint(1, hi)
        return [("key", d) for d in str(n)]
    return []


MOTIONS = [
    [("key", "h")], [("key", "j")], [("key", "k")], [("key", "l")],
    [("key", "w")], [("key", "b")], [("key", "e")],
    [("key", "0")], [("key", "$")], [("key", "%")],
    [("key", "G")], [("key", "g"), ("key", "g")],
]


def gen_motion(rng):
    mot = rng.choice(MOTIONS)
    # `{count}%` is Vim's go-to-percentage-of-file motion, which Garden doesn't
    # implement (its `%` is bracket-match only). Skip the count there to avoid
    # flagging that known unimplemented feature.
    if mot == [("key", "%")]:
        return mot
    return maybe_count(rng) + mot


def gen_operator(rng, allow_enter):
    op = rng.choice(["d", "c", "y"])
    toks = maybe_count(rng)
    toks.append(("key", op))
    if rng.random() < 0.4:
        toks.append(("key", op))  # doubled: dd / cc / yy
    else:
        toks += gen_motion(rng)
    if op == "c":
        toks += gen_insert_text(rng, allow_enter) + [("esc",)]
    return toks


def gen_edit(rng, allow_enter):
    # NB: p / P are NOT here — a bare paste reads Garden's register, which
    # persists across test resets (unlike a fresh Vim process), so it would
    # paste stale content. Paste parity is covered by the self-priming `paste`
    # tier instead.
    kind = rng.choice(["x", "D", "C", "s", "S", "J", "r"])
    if kind == "x":
        return maybe_count(rng) + [("key", "x")]
    if kind == "D":
        return [("key", "D")]
    if kind == "C":
        return [("key", "C")] + gen_insert_text(rng, allow_enter) + [("esc",)]
    if kind == "s":
        return maybe_count(rng) + [("key", "s")] + gen_insert_text(rng, allow_enter) + [("esc",)]
    if kind == "S":
        return maybe_count(rng) + [("key", "S")] + gen_insert_text(rng, allow_enter) + [("esc",)]
    if kind == "J":
        return maybe_count(rng) + [("key", "J")]
    if kind == "r":
        return maybe_count(rng, p=0.2, hi=3) + [("key", "r"), ("key", rng.choice(list("abXY.() ")))]
    return []


def _nonempty_first_line(rng, init):
    """Guarantee a non-empty first line so col-0-forward primes/edits always
    affect the buffer (and thus the register / undo stack) deterministically."""
    if not init or init[0] == "":
        init = [rng.choice(WORDS) + " " + rng.choice(WORDS)] + (init[1:] if init else [])
    return init


def gen_paste_program(rng, allow_enter, allow_open, allow_undo):
    """A self-contained paste test. The prime must ALWAYS refresh the register
    (Garden's register persists across resets, unlike a fresh Vim process), so
    primes are restricted to ops that, from column 0 of a non-empty first line,
    are guaranteed to yank/delete something in *both* editors — no no-op yanks
    (`yb` at col 0) and no operator+motion combos Garden mis-composes (`yG`)."""
    prime = rng.choice([
        [("key", "y"), ("key", "y")],                       # linewise: yank line
        maybe_count(rng, hi=3) + [("key", "y"), ("key", "y")],
        [("key", "d"), ("key", "d")],                       # linewise: delete line
        [("key", "y"), ("key", "w")],                       # charwise forward
        [("key", "y"), ("key", "e")],
        [("key", "y"), ("key", "l")],
        [("key", "d"), ("key", "w")],
        [("key", "x")],                                     # charwise single char
    ])
    moves = [prime]
    for _ in range(rng.randint(0, 2)):
        moves.append(gen_motion(rng))
    moves.append(maybe_count(rng, p=0.3, hi=3) + [("key", rng.choice(["p", "P"]))])
    return [mv for mv in moves if mv]


def gen_undo_program(rng, allow_enter, allow_open, unused):
    """A self-contained undo/redo test: perform k edits that EACH always create
    exactly one undo step (insert-based, so they mutate regardless of buffer
    state), then at most k undos (never reaching past the program's own edits
    into the reset), then at most that many redos."""
    def one_edit():
        # A / I only: their forward behavior already matches Vim (no autoindent
        # difference like o/O), so any divergence AFTER undo/redo is squarely an
        # undo bug rather than a pre-existing content difference.
        entry = rng.choice(["A", "I"])
        # at least one non-space char so the edit is never empty
        txt = [("key", rng.choice("abcXY0"))] + gen_insert_text(rng, allow_enter)
        return [("key", entry)] + txt + [("esc",)]

    k = rng.randint(1, 3)
    edits = [one_edit() for _ in range(k)]
    moves = list(edits)
    nu = rng.randint(1, k)
    for _ in range(nu):
        moves.append([("key", "u")])
    for _ in range(rng.randint(0, nu)):
        moves.append([("ctrlr",)])
    return moves


def gen_insert(rng, allow_enter, allow_open):
    entries = ["i", "a", "I", "A"]
    if allow_open:
        entries += ["o", "O"]
    entry = rng.choice(entries)
    return [("key", entry)] + gen_insert_text(rng, allow_enter) + [("esc",)]


def gen_visual(rng, allow_enter):
    toks = [("key", rng.choice(["v", "V"]))]
    for _ in range(rng.randint(1, 2)):
        toks += gen_motion(rng)
    # NB: '>' / '<' deliberately excluded — Garden indents with 4 spaces while
    # default Vim uses a tab at shiftwidth 8; that is a known design choice, not
    # a parity bug, and would just produce one guaranteed noise cluster.
    op = rng.choice(["d", "y", "x", "~", "u", "U", "J", "c"])
    toks.append(("key", op))
    if op == "c":
        toks += gen_insert_text(rng, allow_enter) + [("esc",)]
    return toks


def gen_undo(rng):
    return [("key", "u")] if rng.random() < 0.6 else [("ctrlr",)]


def gen_program(rng, allow_enter, allow_open, allow_undo):
    """Return a list of *moves*; each move is a token list (a semantic unit).

    Keeping moves grouped lets the minimizer drop whole moves without ever
    re-purposing insert-mode payload (e.g. a typed Space or ')') into a
    normal-mode command — which would manufacture fake divergences.
    """
    kinds = ["motion", "operator", "edit", "insert", "visual"]
    weights = [3, 4, 3, 3, 3]
    if allow_undo:
        kinds.append("undo")
        weights.append(1)
    nmoves = rng.choices([1, 2, 3, 4], weights=[2, 4, 3, 2])[0]
    moves = []
    for _ in range(nmoves):
        k = rng.choices(kinds, weights=weights)[0]
        if k == "motion":
            moves.append(gen_motion(rng))
        elif k == "operator":
            moves.append(gen_operator(rng, allow_enter))
        elif k == "edit":
            moves.append(gen_edit(rng, allow_enter))
        elif k == "insert":
            moves.append(gen_insert(rng, allow_enter, allow_open))
        elif k == "visual":
            moves.append(gen_visual(rng, allow_enter))
        elif k == "undo":
            moves.append(gen_undo(rng))
    return [mv for mv in moves if mv]


def flatten(moves):
    return [t for mv in moves for t in mv]


# ---------------------------------------------------------------------------
# Comparison
# ---------------------------------------------------------------------------


def norm_mode(m):
    m = m.lower()
    if m.startswith("n"):
        return "normal"
    if m.startswith("i"):
        return "insert"
    if m[:1] in ("v", "\x16") or "visual" in m:
        return "visual"
    return m


def diff_flags(vim_r, gar_r):
    """Return set of differing aspects: {'lines','cursor','mode'} (empty == match)."""
    flags = set()
    if vim_r["lines"] != gar_r["lines"]:
        flags.add("lines")
    if list(vim_r["cursor"]) != list(gar_r["cursor"]):
        flags.add("cursor")
    if norm_mode(vim_r["mode"]) != norm_mode(gar_r["mode"]):
        flags.add("mode")
    return flags


# ---------------------------------------------------------------------------
# Delta-debug minimization
# ---------------------------------------------------------------------------


def minimize(garden, init, moves, target_flags):
    """Greedily drop whole *moves* while the program still diverges the same way.

    Operating on moves (not raw tokens) keeps each insert/change move's typed
    payload intact, so minimization can't turn inserted text into a command.
    """

    # Reduce toward *any* divergence, not the exact same flag set: this shrinks
    # a noisy multi-op program down to the single sub-behavior that actually
    # differs (the root cause), which is what we want to read and file.
    def diverges(mvs):
        prog = flatten(mvs)
        if not prog:
            return False
        vim_r = run_vim_case(init, prog)
        if not vim_r.get("ok", True):
            return False
        gar_r = garden.run(init, prog)
        return bool(diff_flags(vim_r, gar_r))

    changed = True
    mvs = list(moves)
    while changed:
        changed = False
        i = 0
        while i < len(mvs):
            cand = mvs[:i] + mvs[i + 1:]
            if diverges(cand):
                mvs = cand
                changed = True
            else:
                i += 1
    # Light within-move trim: try dropping a leading count prefix and any
    # trailing typed characters, since those are always safe reductions.
    def trim_move(mv_idx):
        nonlocal mvs
        mv = mvs[mv_idx]
        # drop leading count digits
        j = 0
        while j < len(mv) and mv[j][0] == "key" and mv[j][1].isdigit():
            cand_mv = mv[j + 1:]
            cand = mvs[:mv_idx] + [cand_mv] + mvs[mv_idx + 1:]
            if cand_mv and diverges(cand):
                mvs = cand
                mv = cand_mv
            else:
                break

    for k in range(len(mvs)):
        trim_move(k)
    return mvs


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--count", type=int, default=300)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--allow-enter", action="store_true",
                    help="allow <CR> in insert mode (autoindent divergence)")
    ap.add_argument("--allow-open", action="store_true",
                    help="allow o/O (autoindent divergence on indented lines)")
    ap.add_argument("--allow-undo", action="store_true",
                    help="(core tier) allow u / <C-R>; over-undo reaches past the reset")
    ap.add_argument("--tier", choices=["core", "paste", "undo"], default="core",
                    help="core: motions/operators/edits/insert/visual; "
                         "paste: self-priming yank+paste; undo: bounded undo/redo")
    ap.add_argument("--out", default=None, help="write JSON report to this path")
    args = ap.parse_args()

    rng = random.Random(args.seed)
    garden = Garden(args.port)

    # sanity: is Garden reachable?
    try:
        garden.state()
    except Exception as e:
        print(f"ERROR: cannot reach Garden debug server on :{args.port}: {e}")
        sys.exit(2)

    # 1) generate
    gen = {"core": gen_program, "paste": gen_paste_program,
           "undo": gen_undo_program}[args.tier]
    cases = []
    for i in range(args.count):
        init = gen_content(rng)
        if args.tier in ("paste", "undo"):
            init = _nonempty_first_line(rng, init)
        moves = gen(rng, args.allow_enter, args.allow_open, args.allow_undo)
        cases.append({"id": i, "init": init, "moves": moves})

    # 2) run vim oracle (one nvim -s process per case, parallelized)
    print(f"running {len(cases)} cases against Vim oracle...", flush=True)
    vim_results = run_vim_batch(cases)

    # 3) run garden, compare
    print("running cases against Garden + comparing...", flush=True)
    divergences = []
    for idx, c in enumerate(cases):
        if idx % 50 == 0:
            print(f"  {idx}/{len(cases)}  (divergences so far: {len(divergences)})", flush=True)
        vim_r = vim_results.get(c["id"])
        if vim_r is None or not vim_r.get("ok", True):
            continue  # vim itself errored on this program; skip
        try:
            gar_r = garden.run(c["init"], flatten(c["moves"]))
        except Exception as e:
            print(f"  garden error on case {c['id']}: {e}", flush=True)
            continue
        flags = diff_flags(vim_r, gar_r)
        if flags:
            divergences.append({"case": c, "vim": vim_r, "garden": gar_r,
                                "flags": sorted(flags)})

    print(f"\nraw divergences: {len(divergences)} / {len(cases)}", flush=True)

    # 4) cluster by coarse signature, minimize one exemplar per cluster
    clusters = {}
    for d in divergences:
        prog = flatten(d["case"]["moves"])
        kinds = tuple(sorted({t[1] if t[0] == "key" else t[0] for t in prog}))
        sig = (tuple(d["flags"]), kinds)
        clusters.setdefault(sig, []).append(d)

    print(f"clusters: {len(clusters)}. Minimizing one exemplar each...", flush=True)
    report = []
    for sig, members in sorted(clusters.items(), key=lambda kv: -len(kv[1])):
        ex = members[0]
        init, moves = ex["case"]["init"], ex["case"]["moves"]
        # Only the core tier is safe to shrink by dropping moves. The paste /
        # undo tiers rely on earlier moves priming the register / undo stack;
        # removing them would re-expose cross-test state (a bare `p` pasting a
        # previous case's yank) and manufacture a fake reproducer.
        if args.tier == "core":
            mini = flatten(minimize(garden, init, moves, set(ex["flags"])))
        else:
            mini = flatten(moves)
        # recompute results on the minimized program for display
        vim_r = run_vim_case(init, mini)
        gar_r = garden.run(init, mini)
        report.append({
            "flags": ex["flags"],
            "count": len(members),
            "init": init,
            "program": pretty(flatten(moves)),
            "minimal_init": init,
            "minimal_program": pretty(mini),
            "vim": {"lines": vim_r["lines"], "cursor": vim_r["cursor"], "mode": norm_mode(vim_r["mode"])},
            "garden": {"lines": gar_r["lines"], "cursor": gar_r["cursor"], "mode": norm_mode(gar_r["mode"])},
        })

    # 5) print report
    print("\n" + "=" * 72)
    print(f"VIM-PARITY REPORT  seed={args.seed} count={args.count}")
    print(f"raw divergences {len(divergences)} in {len(clusters)} clusters")
    print("=" * 72)
    for r in sorted(report, key=lambda r: -r["count"]):
        print(f"\n### [{','.join(r['flags'])}]  x{r['count']}   minimal: {r['minimal_program']}")
        print(f"    init:   {r['minimal_init']}")
        print(f"    keys:   {r['minimal_program']}")
        print(f"    vim   : lines={r['vim']['lines']} cursor={r['vim']['cursor']} mode={r['vim']['mode']}")
        print(f"    garden: lines={r['garden']['lines']} cursor={r['garden']['cursor']} mode={r['garden']['mode']}")

    if args.out:
        with open(args.out, "w") as f:
            json.dump({"seed": args.seed, "count": args.count,
                       "raw": len(divergences), "clusters": report}, f, indent=2)
        print(f"\nJSON report written to {args.out}")


if __name__ == "__main__":
    main()
