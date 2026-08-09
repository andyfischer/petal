# Vim-parity differential fuzzer

A fuzzer that hunts for places where Garden's Vim emulation behaves differently
from **real Vim**. It generates random keystroke programs drawn *only from the
Vim subset Garden implements*, runs each program against both editors from an
identical starting buffer, and reports every disagreement. Because the
generator never emits a key Garden doesn't support, a disagreement is by
construction a Garden parity bug.

```
fuzz.py ──▶ generate keystroke program (supported subset only)
        ├─▶ real Vim   (nvim -s, one hermetic process per case)   ── oracle
        ├─▶ Garden     (headless, driven over the debug server)
        └─▶ compare buffer + cursor + mode → cluster → delta-debug → report
```

## Running

Start Garden headless with a debug server on some free port, pointed at a
throwaway file so there is exactly one focused editor pane:

```bash
cargo run -p garden-app -- --headless --debug-port 8091 /tmp/scratch.txt &
```

Then run a sweep (needs `nvim` on PATH; `python3`, no third-party deps):

```bash
python3 scripts/vim-parity/fuzz.py --port 8091 --count 500 --seed 1
python3 scripts/vim-parity/fuzz.py --port 8091 --count 200 --seed 1 --tier paste
python3 scripts/vim-parity/fuzz.py --port 8091 --count 200 --seed 1 --tier undo
```

`--out report.json` writes the full clustered report as JSON. `--seed` makes a
run reproducible. See `FINDINGS.md` for the bugs a first pass turned up.

### Tiers

- **core** (default): motions (`h j k l w b e 0 $ % gg G`), operators (`d c y`
  + motion / doubled), single-key edits (`x s S D C r J`), Insert entry
  (`i a I A`), charwise/linewise Visual, and counts.
- **paste**: self-contained `p`/`P` tests. Each program primes the register
  with an in-program yank/delete before pasting.
- **undo**: self-contained `u` / `<C-R>` tests. Each program does *k* edits then
  at most *k* undos, so undo never reaches past the program's own changes.

## Why it is built the way it is

Getting a *trustworthy* oracle was most of the work. The non-obvious decisions:

- **`feedkeys(keys, 'ntx')`, one process per case.** The keys are processed
  *as if typed interactively* (`t`), with no remapping (`n`), and the call
  blocks until the whole typeahead is consumed (`x`). This is the faithful model
  of a user typing: a motion that beeps (e.g. `b` at column 0) does **not**
  discard the following keystrokes, and pending operator/count state carries
  across keys. One process per case also makes cases hermetic — no
  mode/register/undo bleed between them.
  - **Why not `nvim -s scriptfile`?** An earlier version of this oracle replayed
    keystrokes with `-s`. It is faithful for motions/operators/paste, but it
    **over-joins the undo history**: it does not break an undo block at
    Insert-mode `<Esc>` the way interactive typing does, so `Ax<Esc>Ay<Esc>u`
    collapses *both* inserts into one block and a single `u` wrongly undoes both.
    That produced a flood of false undo-tier divergences (Garden was right, the
    oracle was wrong). `feedkeys('ntx')` reproduces real vim's per-insert-session
    undo blocks. The two oracles were cross-checked and **agree on 300 core + 300
    paste cases**, differing only on the undo tier — run `oracle_xcheck.py
    {core,paste,undo}` to reproduce. (`nvim_feedkeys(...,'x')` — the RPC form
    without `t` — is a third option that gets the beep/isolation cases wrong; the
    vimscript `feedkeys` with the `t` flag does not.)
- **`-u NONE` is too bare — it flips `startofline` OFF.** Real Vim lands
  `gg`/`G`/`dd`/`C` on the first non-blank; `-u NONE` keeps the column. The
  oracle sets `startofline` back on (and `whichwrap=` to match Garden's
  deliberate non-wrapping `h`/`l`/Space/`BS`). Without this fix the oracle
  disagreed with real Vim and produced a flood of false cursor divergences.
- **Move-aware delta-debugging.** Each divergence is minimized by dropping whole
  semantic *moves*, never splitting a move. Splitting at the token level would
  turn insert-mode payload (a typed Space or `)`) into a normal-mode command and
  manufacture fake findings.
- **Cross-test state is fenced off, not ignored.** Garden reuses one pane, so
  its unnamed register and undo history persist across the buffer reset (a fresh
  Vim process has neither). `p`/`P`/`u`/`<C-R>` are therefore excluded from the
  core tier and moved to dedicated tiers whose programs establish their own
  register / undo state before using it.

## Known intentional differences (excluded, not bugs)

Documented Garden design choices that the generator deliberately avoids so they
don't drown out real bugs: Tab inserts 4 spaces; `>`/`<` indent by 4 spaces
(Vim uses a tab at `shiftwidth`); `o`/`O` and Enter-in-insert copy the current
indent (Vim's default `autoindent` is off — gate these back on with
`--allow-open` / `--allow-enter`); `-` opens the directory browser; `{count}%`
is go-to-percentage in Vim but bracket-match only in Garden.
