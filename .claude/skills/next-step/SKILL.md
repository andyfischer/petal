---
name: next-step
description: Reads the project goals and documentation, assesses current status, picks the highest-impact next task, and implements it using TDD. Use when the user wants to make progress on the project without specifying a particular task.
---

# Next Step

Autonomously pick and implement the highest-impact next task for this project.

## Step 1: Read Project Context

Read the project's goals and documentation to understand the vision and current priorities:

1. Read `docs/goals.md` (the primary goals document — vision and remaining work).
2. Read all `.md` files in the project root directory (e.g. any `Goals_` documents and etc).
3. Read `CLAUDE.md` for project conventions, testing instructions, and build commands.

Summarize your understanding of the project's goals and current priorities before proceeding.

## Step 2: Assess Current Status

Check the current state of the codebase:

1. Run `git log --oneline -20` to see recent work.
2. Run `git status` to check for any in-progress changes.
3. Run the test suite (`npx vitest --run`) to see what's passing and failing.
4. Briefly review any TODO comments or known issues mentioned in the docs.

Summarize what's working, what's broken, and what areas need attention.

## Step 3: Pick the Highest-Impact Task

Based on the goals, documentation, and current status, identify the single highest-impact task to work on next. Consider:

- Failing tests or broken functionality (fix these first).
- Features explicitly called out in the goals doc that aren't yet implemented.
- Gaps between the documented vision and the current implementation.

State which task you've chosen and **why** it's the highest impact. Ask the user to confirm before proceeding.

## Step 4: Confirm Existing Behavior

Before making changes, establish a baseline:

1. Run relevant tests to confirm their current pass/fail status.
2. If the area you're working on lacks test coverage, note this — you'll add tests in the next step.

## Step 5: Implement with TDD

Use a test-driven development approach:

1. **Write a failing test first** — add a unit or integration test that captures the expected behavior for the task you've chosen. Run the test suite and confirm it fails as expected.
2. **Implement the fix or feature** — write the minimum code needed to make the failing test pass.
3. **Run all tests** — confirm the new test passes and no existing tests have regressed.

If TDD doesn't fit the task (e.g. documentation-only changes), skip the failing-test step but still run the full test suite before and after your changes.

## Step 6: Final Verification

1. Run the full test suite one final time to confirm everything passes.
2. If there are example programs (`examples/*.ptl`), run a quick smoke test with `./bin/test-each.sh` to check for regressions.

## Step 7: Update docs

If you are working on an item from a 'goals' document, then update the document with new comments to reflect the latest progress.

## Step 8: Commit

Once everything is green:

1. Stage the relevant files.
2. Create a commit with a clear, descriptive message summarizing what was done and why.
