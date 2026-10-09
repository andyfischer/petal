# Open work, ranked

Status: 2026-10-08, checked against `47d1e0e`. The ten highest-impact
unresolved items from [todo-bugs-20260923.md](todo-bugs-20260923.md),
[testbed-takeaways.md](testbed-takeaways.md), the follow-ups of the
2026-10-08 `var`/`bench`/`-> nil`/`pub` work, and the 2026-10-06 testbed
debriefs (email client, network graph, tower defense). Each was reproduced
with a release build of HEAD unless it says otherwise.

Ranked: what blocks a release, then silent wrong results, then what every
new app or newcomer trips on.

## 1. The install path fails and nothing says 1.0

`https://petal-lang.org/install.sh` and the GitHub `releases/latest` tarball
both return 404, and the installer is the first command in the README.
`petal --version` prints `0.1.0`, the README says "early, experimental phase",
and there is no CHANGELOG. Cut a release with `release-petal.yml`, deploy the
site, test the one-liner on a clean machine. Details: todo-bugs §1.1, §1.4.

## 2. Recursion under the default policy uses hundreds of MB
DONE petal run leaves memoization off unless a policy is named; memo scopes stop 96 frames deep and the table is bounded at 96 MB of records; MAX_CALL_DEPTH is 20,000 (path-keyed state keeps 5,000); fib(30) and depth-10k under an RSS ceiling in test/vitest/recursion-memory.test.ts

`fib(27)` takes 541 MB and 0.47 s under `fast`, against 4.5 MB and 0.04 s
under `--policy baseline`. Linear recursion is quadratic in depth under `fast`
(30 MB at 1,000, 100 MB at 2,000, 479 MB at 4,900). The frame-path copy is
fixed, so what is left is the memo records. Bound the memo table or skip
memoizing in `petal run`, add `fib(30)` and a depth-10k case to the perf suite
with a memory ceiling, then raise `MAX_CALL_DEPTH` from 5,000. Details:
todo-bugs §1.2, §2.

## 3. `check --strict` passes programs that are silently wrong
DONE check warns on a leading-`-` broken continuation, a discarded bare computation, a variant declared by two enums or named like a top-level let/state/fn/class, and a call that reaches a let/var/state shadowing a builtin; an enum name is accepted as a type name (typecheck/unused.rs, typecheck/shadow.rs)

- A leading `-` on a continuation line starts a new statement, so a sum
  written down the page returns its last line. No warning from `check` or
  `lint`.
- Two enums that declare the same variant, and a variant that overrides a
  `let`, compile with no diagnostic.
- A `let`/`state` that shadows a builtin called in the same file (`state
  split = 396`, then `split(text, ",")`) gets the "captured `state`" warning,
  which never says a builtin is shadowed (email client).
- An enum name as a return type warns `unknown type name`.

Details: todo-bugs §3.

## 4. `len` and `slice` count bytes and cut characters silently
DONE len/slice/s[i]/.length count code points (index_of already did); byte_len/byte_slice added; heap strings carry a code-point count so ASCII is O(1) and a non-ASCII scan is linear; downstream notes in code-point-migration.md

`slice("Óscar", 0, 1)` is `""` and `len("Óscar")` is 6. Move `len`, `slice`,
`s[i]` and `index_of` to code points, keep `byte_len`/`byte_slice`, and sweep
the ecosystem's `.ptl` files first. It needs the ASCII fast path in
[char-at-fast-path](optimizations/char-at-fast-path.md) to stay O(1); `char_at`
is still two O(n) walks and an allocation per call. Details:
testbed-takeaways §1.

## 5. The guides mislead on `state`, dictionaries and errors
DONE petal run --state-storage <file> keeps state between runs as JSON, and a script with state warns on stderr without it (docs/CLI.md "State between runs"); guides left as-is per the PLAN

- `state hits = 0; hits += 1; print(hits)` is still captioned "1 on the first
  run, 2 on the second" in `writing-petal-guide.md:238` and
  `language-guide.md:1984`. Separate `petal run` processes print 1 each time.
  Show state working within one run and say what a "run" is.
- Neither guide says a record is the dictionary (`r[k] = v`, string keys
  only), how a program reports or handles an error, what is truthy, or that
  float division by zero is an error.
- "Method syntax reaches builtins, not your own functions" reads as "class
  methods don't work with `.`".

Details: todo-bugs §1.3, §5, §7.

AUTHOR NOTE: That syntax works correctly when talking about Petal scripts
that are used in our interactive harnesses like petal-sdl, since
they preserve `state` values from frame to frame. However it doesn't
work correctly when running from the CLI because the CLI does not do that.

PLAN: Let's leave the simple README and guides as-is to not overcomplicate things.

Let's add support to the CLI for this. The CLI should support params like
--state-storage <filename> which (if provided) stores all the state values
as JSON in that file, creating the file if needed. If the CLI is called
multiple times with the same state storage file, then the state example
should work correctly.

Additionally, if the CLI is used to execute a script that has any `state`,
and if the --state-storage option is not used, then the tool should
print a warning to the console that state will not work correctly,
and they should probably re run the command with --state-storage.

## 6. One test is red and the UI goldens are not trusted
DONE gate was right, the test misread clock-driven apps (renamed a_quiet_corpus_idles_unless_it_reads_the_clock); bloom focus ring off frame_count(); 6 stale goldens were state-label ordinals only, 14 missing entries added; bold ui text_metrics fixed; left for core: `let x = for … end` is not observed (lower.rs emit_counted_loop leaves cur_origin on the body)

- `petal-ui` test `gating::a_quiet_corpus_mostly_idles` fails: 18 of 38 apps
  idle after 90 quiet frames.
- Six bloom-using apps have stale hashes in `test/ui-golden`, and
  `physics-playground/app.ptl` has no entry in its index (not re-run today).
- `garden/target/debug/garden` dates from 2026-09-24. The tower defense,
  email client and network graph apps have only ever been run on a binary
  that predates default parameters and named arguments. Rebuild and re-run
  them; check on the way whether `let x = for … end` is still missing from
  `panel.values` and whether `text_metrics(style).baseline` matches the `ui`
  face at weight 700.
- Not run since the 2026-10-08 merge: the golden corpus (`test-examples.ts`),
  `petal check` parity over `~/.garden` and `~/worlds-fair/ui/ptl`, the full
  garden-app suite, and the fantasy-nes tests (missing libjxl).

The vitest suite passes at HEAD (1,403 tests) once dependencies are installed
and `core-libs/petal-ui` has a debug `petal-ui-run`.

## 7. The headless panel test loop is not deterministic or documented right
DONE dt() is virtual on a ticked panel (and across /panel/reset); POST /mouse takes "pane"; tools/panel-test.sh; skill step 4 runs check --host garden and lint

- `dt()` stays on the wall clock after `POST /tick`, while `time()` goes
  virtual. A sim stepped by `dt()` drifts whenever a test sends input between
  ticks. AUTHORING.md still says "drive animation off `dt()`". Put `dt()` on
  the virtual clock once a panel is ticked, or document the `time()`-delta
  idiom and the warm-up tick.
- `docs/skills/write-example-app.md` step 4 runs `check --strict` without
  `--host garden`, which reports `claim_key` and `request_frame` as unknown on
  a correct panel script. It also never runs `petal lint`. Four debriefs hit
  this.
- Every session rewrites the same shell helpers (`click`, `key`, `tick`,
  `obs`, `shot`) and gets the pane offset wrong. Ship one under `tools/`, or
  add a pane-local option to `POST /mouse`.

## 8. Missing string basics, and `++` in a loop is O(n²)
DONE repeat/starts_with/ends_with/trim builtins; strings over 512 bytes are no longer interned, so the 100k `++` loop is 4.16 s -> 0.10 s (still quadratic in bytes copied, not amortized O(1)); guide §5 recommends repeat and join; lint rule prefer-repeat

`repeat`, `starts_with`, `ends_with` and `trim` are all `Unknown builtin`;
the email client and both simulated users wrote their own. `out = out ++ "x"`
takes 3.8 s at 100k iterations, and the usage guide §5 recommends that loop.
Add the four builtins, and make append to a uniquely held string amortized
O(1) or point people at `join`. Details: todo-bugs §4.1.

## 9. CLI argument handling turns typos into file errors
DONE unknown commands and options are named; check keeps --host ui and run explains ui names; --strict ignores the export deprecation; unclosed blocks and cross-module traces fixed (core/tests/cli_args.rs)

- `petal repl` and `petal test` print `Error reading file 'repl'`.
- An unknown flag is taken as the file path: `petal run x.ptl --iter 3` gives
  `Error reading file '3'` (shared `parse_source_args`, so `bench` too).
- `check` defaults to `--host ui` and passes a script that `run` rejects with
  `Undefined variable: ui`.
- `check --strict` fails on the `export` deprecation warning, and `run`
  prints it. Decide whether that is wanted.
- An unclosed block does not name its opener, and a cross-module stack trace
  drops the file name of the call site.

Details: todo-bugs §6.

## 10. Text editing is hand-rolled in every app
DONE text_field has selection, cut/copy/paste (prelude clipboard_get/clipboard_set over a host clipboard channel) and undo/redo; text_wrap_rows, text_row_of and text_range_rects work in code-point offsets; the email client's search, To and Subject fields use text_field_update (core-libs/petal-ui/docs/components.md)

`text_field` has no selection, clipboard or undo
(`core-libs/petal-ui/prelude/ui.ptl:2264`). `text_wrap` returns strings
without offsets, and there is no "rects for this character range" call, so a
wrapped editable area or a search highlight measures one growing prefix per
character. The email client carries five copies of the same field plumbing.
Add selection and clipboard to `text_field`, a `text_wrap` variant returning
`{text, start, end}` per row, and `text_range_rects(s, style, a, b)`.
Details: testbed-takeaways §2.
