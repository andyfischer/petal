#!/usr/bin/env python3
"""Cross-check the two candidate vim oracles against each other (no Garden).

The fuzzer's oracle is `feedkeys(..., 'ntx')` (see `fuzz.run_vim_case`). An
earlier version used `nvim -s` keystroke replay. This tool re-runs both and
reports where they disagree, documenting *why* the switch was made:

  - core / paste : they agree everywhere (`-s` is faithful there), so the swap
                   is safe.
  - undo         : they diverge, because `-s` over-joins undo blocks across an
                   Insert-mode `<Esc>` (a single `u` wrongly undoes two inserts).
                   `feedkeys('ntx')` matches real interactive vim.

Usage:  oracle_xcheck.py {core|paste|undo} [count] [seed]
"""
import os, sys, tempfile, subprocess, random
sys.path.insert(0, os.path.dirname(__file__))
import fuzz


def run_vim_s(init, program):
    """The retired `nvim -s` keystroke-replay oracle, kept here so the oracle
    comparison stays reproducible."""
    body = fuzz.to_vim_bytes(program)
    with tempfile.TemporaryDirectory() as d:
        content = os.path.join(d, "buf.txt")
        keysf = os.path.join(d, "keys")
        lout = os.path.join(d, "lines")
        cout = os.path.join(d, "cur")
        with open(content, "w") as f:
            f.write("\n".join(init) + "\n")
        keys = body + "\x1b"
        keys += ":call writefile(getline(1,'$'), $VP_L)\r"
        keys += ":call writefile([(line('.')-1).' '.(col('.')-1)], $VP_C)\r"
        keys += ":qa!\r"
        with open(keysf, "wb") as f:
            f.write(keys.encode("latin-1"))
        env = dict(os.environ, VP_L=lout, VP_C=cout)
        try:
            subprocess.run(
                ["nvim", "--headless", "-u", "NONE", "-n", "-i", "NONE",
                 "-c", "set noswapfile noautoindent nosmartindent noexpandtab "
                       "startofline whichwrap=",
                 "-s", keysf, content],
                env=env, check=True, capture_output=True, timeout=15,
            )
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
            return {"lines": None, "cursor": None, "mode": "n", "ok": False, "err": str(e)}
        if not (os.path.exists(lout) and os.path.exists(cout)):
            return {"lines": None, "cursor": None, "mode": "n", "ok": False, "err": "no output"}
        with open(lout, encoding="utf-8", errors="replace") as f:
            raw = f.read()
        lines = raw.split("\n")
        if lines and lines[-1] == "":
            lines = lines[:-1]
        with open(cout) as f:
            cl, cc = f.read().split()
    return {"lines": lines, "cursor": [int(cl), int(cc)], "mode": "n", "ok": True}


def main():
    tier = sys.argv[1]
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 300
    seed = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    rng = random.Random(seed)
    gen = {"core": fuzz.gen_program, "paste": fuzz.gen_paste_program,
           "undo": fuzz.gen_undo_program}[tier]
    disagree = 0
    examples = []
    for i in range(count):
        init = fuzz.gen_content(rng)
        if tier in ("paste", "undo"):
            init = fuzz._nonempty_first_line(rng, init)
        moves = fuzz.flatten(gen(rng, False, False, False))
        s = run_vim_s(init, moves)               # retired -s replay oracle
        fk = fuzz.run_vim_case(init, moves)      # current feedkeys('ntx') oracle
        if not (s.get("ok") and fk.get("ok")):
            continue
        if s["lines"] != fk["lines"] or s["cursor"] != fk["cursor"]:
            disagree += 1
            if len(examples) < 10:
                examples.append((fuzz.pretty(moves), init, s, fk))
        if i % 50 == 0:
            print(f"  {i}/{count}  -s vs feedkeys disagreements:{disagree}", flush=True)
    print(f"\n[{tier}] -s vs feedkeys('ntx') disagreements: {disagree}/{count}")
    for pr, init, s, fk in examples:
        print(f"  {pr!r} init={init}")
        print(f"     -s (retired): {s['lines']} cur={s['cursor']}")
        print(f"     feedkeys    : {fk['lines']} cur={fk['cursor']}")


if __name__ == "__main__":
    main()
