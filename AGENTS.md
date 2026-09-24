See these important files:

README.md - project outline
docs/dev/scripts.md - developer tools and scripts
docs/dev/testing.md - how testing is handled
docs/dev/performance.md - profiling tools, what the optimizer does, where the headroom is
docs/dev/mcp-server.md - using the local MCP server to do testing and investigation
docs/dev/formal-verification.md - Kani proofs and exhaustive small-scope checks of the core semantics

## Commits

Save a git commit after each chunk of work (a finished task, a fix, or a
self-contained step) without waiting to be asked. Stage the specific files you
changed rather than `git add -A`. If a pre-commit hook modifies files, re-stage
them and retry once.

Message style: Conventional Commits, `type(scope): subject`, lowercase,
imperative, one line. Types are `feat`, `fix`, `perf`, `refactor`, `docs`,
`style`, `test`. The scope is optional and names a module or component (`vm`,
`garden`, `check`, `petal-c-bridge`). Join related changes with semicolons.

 - `fix(vm): stop runaway recursion with a stack overflow error`
 - `perf(vm): cheaper call setup; pb_vm_set_memo host switch`

Re-baseline the ui-golden files when an upstream change affects them.

PR title: Conventional Commits format. PR body: a summary of the changes, key
implementation details, and a test plan.
