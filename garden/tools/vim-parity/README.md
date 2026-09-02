# Vim-parity differential fuzzer

A fuzzer that hunts for places where Garden's Vim emulation behaves differently
from **real Vim**. It generates random keystroke programs drawn *only from the
Vim subset Garden implements*, runs each program against both editors from an
identical starting buffer, and reports every disagreement. Because the
generator never emits a key Garden doesn't support, a disagreement is by
construction a Garden parity bug.

```
fuzz.ts ──▶ generate keystroke program (supported subset only)
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

Then run a sweep (needs `nvim` on PATH; Node >= 22.6, no third-party deps):

```bash
node tools/vim-parity/fuzz.ts --port 8091 --count 500 --seed 1
node tools/vim-parity/fuzz.ts --port 8091 --count 200 --seed 1 --tier paste
node tools/vim-parity/fuzz.ts --port 8091 --count 200 --seed 1 --tier undo
```

`--out report.json` writes the full clustered report as JSON. `--seed` makes a
run reproducible.

### Tiers

- **core** (default): motions (`h j k l w b e 0 $ % gg G`), operators (`d c y`
  + motion / doubled), single-key edits (`x s S D C r J`), Insert entry
  (`i a I A`), charwise/linewise Visual, and counts.
- **paste**: self-contained `p`/`P` tests. Each program primes the register
  with an in-program yank/delete before pasting.
- **undo**: self-contained `u` / `<C-R>` tests. Each program does *k* edits then
  at most *k* undos, so undo never reaches past the program's own changes.

## How the oracle works

Getting a trustworthy oracle was most of the work. The decisions that matter:

- **`feedkeys(keys, 'ntx')`, one nvim process per case.** Keys are processed
  as if typed interactively (`t`), with no remapping (`n`), and the call blocks
  until the typeahead is consumed (`x`). This matches a real user: a motion
  that beeps (`b` at column 0) does not discard the following keys, and
  pending operator/count state carries across keys. One process per case keeps
  cases hermetic.
  - `nvim -s scriptfile` was tried first. It is faithful for motions,
    operators, and paste, but it does not break an undo block at Insert-mode
    `<Esc>`, so `Ax<Esc>Ay<Esc>u` wrongly undoes both inserts. That produced
    false undo-tier divergences. `node tools/vim-parity/oracle-xcheck.ts
    {core,paste,undo}` cross-checks the two oracles; they agree on core and
    paste and differ only on undo.
- **`-u NONE` turns `startofline` off.** Real Vim lands `gg`/`G`/`dd`/`C` on
  the first non-blank; `-u NONE` keeps the column. The oracle sets
  `startofline` back on, and `whichwrap=` to match Garden's non-wrapping
  `h`/`l`/Space/`BS`.
- **Move-aware delta-debugging.** Each divergence is minimized by dropping
  whole semantic moves, never splitting one. Splitting at the token level would
  turn insert-mode text into normal-mode commands and manufacture fake
  findings.
- **Cross-test state is fenced off.** Garden reuses one pane, so its register
  and undo history persist across the buffer reset, while a fresh Vim process
  has neither. `p`/`P`/`u`/`<C-R>` are therefore excluded from the core tier
  and given their own tiers, whose programs set up their own register and undo
  state first.

## Known intentional differences (excluded, not bugs)

Documented Garden design choices that the generator deliberately avoids so they
don't drown out real bugs: Tab inserts 4 spaces; `>`/`<` indent by 4 spaces
(Vim uses a tab at `shiftwidth`); `o`/`O` and Enter-in-insert copy the current
indent (Vim's default `autoindent` is off — gate these back on with
`--allow-open` / `--allow-enter`); `-` opens the directory browser; `{count}%`
is go-to-percentage in Vim but bracket-match only in Garden.
