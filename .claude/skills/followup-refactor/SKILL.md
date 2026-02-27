---
name: followup-refactor
description: Cleans up common issues in newly generated code. Use this when preparing code for a code review or when improving the project's code quality.
---

# Followup Refactor

Perform a high-impact refactor, code cleanup, or reorganization of the codebase.

If the current session just worked on a new feature, focus the refactor on that new code first (if it needs it), then broaden to the rest of the codebase.

## Step 1: Understand Recent Context

1. Run `git log --oneline -20` to see recent work.
2. Run `git diff HEAD~3 --stat` to see what files changed recently.
3. If there were recent changes in this session, read the changed files to understand the new code.

Summarize what was recently worked on.

## Step 2: Identify Refactoring Opportunities

Explore the codebase looking for high-impact refactoring opportunities. Consider each of these categories:

### Pain points slowing down development
- Are there awkward APIs or patterns that make common tasks harder than they should be?
- Are there brittle areas that break often or are hard to modify?

### Simplification
- Is there code doing something in a complex way that could be simpler?
- Are there unnecessary abstractions, over-engineered patterns, or dead code?
- Are there functions or methods that are doing too much?

### Duplication
- Are there places where similar logic is copy-pasted that could be extracted into a shared function?
- Are there repeated patterns that would benefit from a helper or utility?

### Consistency
- Are there similar concepts handled in different ways across the codebase?
- Are there inconsistent naming conventions, data representations, or coding patterns?
- Are there places where similar data types could be unified?

### Complexity and organization
- Are there files that are too large and should be split?
- Are there functions that are too long or deeply nested?
- Does the directory structure still make sense as the codebase grows?
- Are there new abstractions or modules that would make the architecture clearer?

Read through the source files, paying special attention to:
- Large files (check file sizes)
- Files with many responsibilities
- Areas touched by recent changes

## Step 3: Prioritize and Propose

Pick the **single highest-impact** refactoring opportunity. High impact means:
- It meaningfully reduces complexity or duplication.
- It makes future development easier.
- It improves code clarity without changing behavior.

Present your proposed refactor to the user:
- What you want to change and why.
- What the before/after looks like conceptually.
- What risks there are (if any).

**Ask the user to confirm before proceeding.**

## Step 4: Establish a Baseline

Before making changes:

1. Run the full test suite (`npx vitest --run`) and note the results.
2. If applicable, run `./bin/test-each.sh` for example program smoke tests.

All tests must be green before you start refactoring. If they aren't, fix failing tests first or flag them to the user.

## Step 5: Refactor

Make the changes. Follow these principles:

- **No behavior changes.** The refactor should be purely structural. If you find a bug, note it but fix it in a separate step.
- **Small, incremental moves.** Prefer a series of small safe transformations over one big rewrite.
- **Run tests frequently.** After each significant change, run the test suite to catch regressions early.
- **Keep the diff reviewable.** Avoid mixing unrelated changes in the same refactor.

## Step 6: Final Verification

1. Run the full test suite (`npx vitest --run`) and confirm everything passes.
2. Run `./bin/test-each.sh` to smoke-test example programs.
3. If the refactor touched public APIs or module boundaries, do a quick grep to make sure nothing was missed.

## Step 7: Commit

Once everything is green:

1. Stage the relevant files.
2. Create a commit with a clear message explaining what was refactored and why.
