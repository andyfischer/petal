# Testing

Petal has tests in the language: a `test` block beside the code it tests,
expectations that record what is wrong and carry on, and `petal test` to find
the blocks and run them.

```petal
fn after_hit(lives: int) -> int
  max(lives - 1, 0)
end

test "a hit costs one life" do
  expect_eq(after_hit(3), 2)
  expect_eq(after_hit(0), 0, "never below zero")
end
```

```console
$ petal test lives.ptl
lives.ptl
  ok    a hit costs one life

1 test: 1 passed (1 file)
```

`petal run lives.ptl` does not run the block, and neither does a host that
runs the file every frame. A test costs an ordinary run nothing but the
definition of one function it never calls.

## Test blocks

```petal ignore
test "name" do
  …
end
```

- A test is written at the **top level** of a file, in any file: an entry
  script, a module, a package. Inside a function, a loop or another block it
  is an error.
- The name is a plain string (no `{…}` holes) and is unique within its file.
- `test` is a keyword only there, at the start of a top-level statement and
  directly before a string. Everywhere else it is an ordinary name: `let test
  = 3`, `fn test()`, `r.test` and a call `test(x)` all still work.
- The body is a function body. It sees everything the file's top level
  declares, private names included, and reads it the way any function does: a
  `let`, a `const` or a function by its bare name, and a module-level `var`
  or `state var` with `get`:

  ```petal
  state var score = 0

  fn award(n: int)
    set score += n
  end

  test "award adds to the score" do
    award(5)
    award(2)
    expect_eq(get score, 7)
  end
  ```

- `return` leaves the test early. What the body evaluates to is ignored.
- `petal fmt`, `petal check`, `petal lint` and the language server treat the
  body as the function body it is: it is indented, type-checked, linted and
  completed like the rest of the file, and the outline lists the test by its
  name.

## Expectations

Three builtins check something and **record** a failure instead of raising:

| | |
|---|---|
| `expect(condition, message?)` | fails when `condition` is `false` or `nil` |
| `expect_eq(actual, expected, message?)` | fails when the two are not `==` |
| `expect_near(actual, expected, tolerance, message?)` | fails when two numbers differ by more than `tolerance` |

A failure is recorded with the file and line of the call and the values that
were compared, and the test goes on to its next line, so one run reports
everything that is wrong. Had `after_hit` been written `lives - 2`:

```console
$ petal test lives.ptl
lives.ptl
  FAIL  a hit costs one life
        lives.ptl:6: expected 2, got 1
        lives.ptl:7: expected 0, got -2 (never below zero)

1 test: 0 passed, 1 failed (1 file)
```

Each of the three answers `true` or `false`, for the test that cannot go on
past a failure:

```petal ignore
if !expect(found != nil, "the level has a spawn point") then return end
expect_eq(found.x, 3)
```

Values are printed so that they cannot be mistaken for each other: a string
is quoted, so `expect_eq("1", 1)` reads `expected 1, got "1"`. `expect_near`
says how far off the value was, and a value that is not a number is never
near anything.

`assert` and `assert_eq` are the other kind of check. They **raise**: the
test stops there and is reported as an error, with the position and a stack
trace, as for any runtime error. Use an expectation for what a test is
checking, and an assertion for what must hold before the rest of the test
means anything.

Expectations work outside tests too. At the top level of a script, or in a
function it calls, a failed expectation is recorded the same way; `petal run`
prints each one on stderr when the run ends (`expectation failed:
game.ptl:12: expected 3, got 4`) and exits 1, and a host reads them from its
`Env` (below).

## `petal test`

```
petal test [--filter <text>] [--json] [--list] [--show-output]
           [--timeout <seconds>] [<path>...]
```

Each path is a file or a directory; the default is the current directory. A
directory means every `.ptl` file under it (dot-directories, `node_modules`
and `target` are skipped), and a file with no test block is passed over
without being loaded.

**What runs.** Each file with tests is loaded as the entry of its own
program. Its imports resolve as `petal run` resolves them: relative files,
the project's `petal.toml` dependencies, `-I` directories and the bundled
packages. Only the tests written **in that file** are run, so a test in a
module runs once, when the module's own file is reached, however many files
import it.

**Isolation.** Every test gets a fresh stack. The file's top level runs
first, so its functions, `var`s and `state` exist with their initial values,
and then the test body. Nothing one test writes is seen by another, and tests
do not depend on the order they are written in. The top level therefore runs
once per test; what it prints is not shown.

**The report.** One line per test, with the failures under it. A test is
`ok`, `FAIL` (an expectation did not hold) or `ERROR` (it raised or timed
out). What a failing test printed is shown under `output:`; `--show-output`
shows it for passing tests too. A failed expectation at the file's top level
is reported once for the file, as `(top level)`.

**Exit status.** 1 when any test fails or errors, when a file with tests
cannot be run, or when a top-level expectation fails; 0 otherwise, including
when no tests were found.

**Options.**

- `--filter <text>` runs only the tests whose name contains the text.
- `--list` prints the tests and runs nothing.
- `--timeout <seconds>` stops a test that runs longer and reports it as an
  error. The default is 60; 0 is no limit.
- `--json` prints one document instead of the report:

  ```json
  {
    "ok": false,
    "summary": { "files": 1, "tests": 1, "passed": 0, "failed": 1,
                 "errors": 0, "file_errors": 0, "top_level_failures": 0 },
    "files": [
      { "file": "lives.ptl", "error": null, "top_level": [],
        "tests": [
          { "name": "a hit costs one life", "line": 5, "status": "failed",
            "checks": 2, "error": null, "output": [], "duration_ms": 0.04,
            "failures": [
              { "check": "expect_eq", "file": "lives.ptl", "line": 6,
                "column": 3, "message": "expected 2, got 1",
                "actual": "1", "expected": "2" } ] } ] } ]
  }
  ```

  `status` is `passed`, `failed` or `error`. A failure has `tolerance` when
  it is an `expect_near` and `note` when the call passed a message.

**Files that need a host.** A file that imports a host's interface (`ui`, a
game engine's package) calls natives the `petal` command does not have, so it
is reported as a file that could not be run. Its tests are run by that host.
Keeping logic in modules that import no host is what makes it testable from
the command line; `examples/games/physics-playground/physics.ptl` is one, and
carries its own tests.

## Running tests from a host

An embedder runs the same blocks with its own natives registered. The pieces
are on `Env` (Rust):

```rust
// The program's tests: the entry file's and every module's.
let tests: Vec<TestDecl> = env.tests(pid).to_vec();
// TestDecl { name, module: Option<String>, file, path, line, function }

// Functions exist once the program has run.
env.run(stack)?;

for test in &tests {
    env.test_begin(stack, &test.function)?;
    let result = loop {
        match env.test_resume(stack) {
            TestStep::Finished(result) => break result,
            // A native asked to wait: run the frames it asked for, then go on.
            TestStep::Suspended => host.run_frames(&mut env, stack),
            TestStep::OutOfSteps => {} // only from test_resume_bounded
        }
    };
    // TestResult { function, passed, failures, failed, checks, error }
    report(test, &result);
}
```

- `Env::tests(program) -> &[TestDecl]` lists the blocks. `function` is the
  top-level function a block compiles to (`__test_0`, `sim::__test_2`);
  `module` is `None` for the entry file, which is how a runner leaves out the
  tests of a library package.
- `Env::test_begin(stack, function)` starts a test. The stack must have
  completed a run and must not be in the middle of one. The test sees the
  program as that run left it; a runner that wants isolation gives each test
  its own stack (`create_stack`, one `run`), or resets the state it cares
  about.
- `Env::test_resume(stack) -> TestStep` runs it until it finishes or is
  suspended. `test_resume_bounded(stack, max_steps)` also stops after about
  that many instructions (`OutOfSteps`), for a runner with a time limit.
- `Env::test_abort(stack, why)` gives up on a waiting test and returns what
  it recorded so far. `Env::run_test(stack, function)` is begin and resume in
  one call, for a runner with nothing to wait for.
- `TestResult::failures` is a list of `ExpectFailure { check, file, line,
  column, message, actual, expected, tolerance, note }`. `error` is the
  runtime error that ended the test, if one did.
- `Env::take_expect_failures(stack)` returns the failures recorded *outside*
  tests: by the program's ordinary runs and by `Env::call_function`. A test's
  failures never land there, and a frame's never land in a test's result.
  `Env::expect_counts(stack)` is how many expectations were evaluated there
  and how many failed. The log holds at most 1000 failures and counts the
  rest, so a check that fails every frame cannot grow it without bound.
- A test's function is an ordinary entry of the stack's function table, so
  `Env::has_function` answers for it and a host that lists a script's
  functions sees `__test_0` among them; `petal::ast::is_test_fn_name` tells
  them apart.

### A test that spans frames

A test body is ordinary code: it runs from its first line to its last. A host
with a frame loop lets it wait in the middle by **suspending** it. The host
registers a native such as `wait_frames(n)` that notes `n` where its loop can
see it and calls `PetalCxt::suspend()`:

```rust
env.register_native_boxed("wait_frames", Box::new(move |cxt: &mut PetalCxt| {
    let n = cxt.get_int(1)?;
    cxt.suspend()?;          // an error outside a test the host is driving
    wanted.set(n);
    cxt.push_nil();
    Ok(1)
}), NativeEffects::EFFECT);
```

The call returns normally, the test stops after it, and `test_resume`
answers `Suspended`. The test's frames are set aside on the stack, which is
then free for the program's ordinary runs: the host runs `n` frames exactly
as it always does (`reset_stack`, bind inputs, `run`, draw) and calls
`test_resume` again. The test carries on from the line after `wait_frames`.

```petal ignore
test "the door opens two seconds after the switch" do
  press_switch()
  wait_frames(120)
  expect(get door_open, "door still closed")
end
```

What the test sees when it resumes is what any function called later sees.
A `var` or `state var` read with `get` is live, so `get door_open` is the
value the frames in between left. A `let` or a plain `state` the test
mentions was captured when the test began and has not moved; read what
changes through `get`, or through a native.

Limits to know:

- `suspend` is an error where nothing can resume the call: in an ordinary
  run, in a function the host called with `call_function`, and inside a
  callback of `map`, `filter`, `reduce`, `forEach` or `sort` (their frames
  are on the native stack). Wait in a `for` loop instead.
- One test at a time per stack.
- A reload of the program while a test is waiting ends the test with an
  error; begin it again on the new program.
- `petal test` has no frames to run, so a test that suspends there is
  reported as an error saying it needs the host.

A host native can record expectations of its own, with the position of the
call that made them: `cxt.fail_expectation("expect_pixel", message)` and
`cxt.pass_expectation()`. `cxt.call_location()` gives the file, line and
column of the calling expression.

## Testing Petal itself

Petal's own tests that are about what a program evaluates to can be written
this way. `test/ptl/` holds them, and the examples carry tests where they
have logic worth holding (`examples/console/fibonacci.ptl`,
`examples/console/text_utils.ptl`, the physics playground's solver). `cargo
test --test ptl_tests` runs them all; see [dev/testing.md](dev/testing.md).
