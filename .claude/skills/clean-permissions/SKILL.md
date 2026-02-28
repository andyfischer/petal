# Skill: Clean Permissions

Analyze Claude Code permission settings and suggest cleanups to reduce clutter and future prompts.

## Trigger

Use when the user says `/clean-permissions` or asks to clean up permission settings.

## Instructions

### Step 1: Read Settings Files

Read all three settings files (skip any that don't exist):
- `.claude/settings.json` (team-shared, committed)
- `.claude/settings.local.json` (personal, gitignored)
- `~/.claude/settings.json` (global user settings)

### Step 2: Gather Permission History

Run `cc-session-history list-permission-checks` to get recent permission check data. This shows which commands are being prompted for and how often.

### Step 3: Analyze and Categorize

Check for these categories of issues:

**Stale rules** — Permission rules referencing paths that don't exist on disk:
- For each `Bash(...)` rule, extract the command/path and check if the referenced binary or script exists
- Flag rules pointing to renamed or deleted directories, binaries, or scripts

**Redundant rules** — Rules subsumed by broader rules in the same or higher-precedence file:
- `Bash(candle start *)` is redundant if `Bash(candle *)` exists
- `Bash(git commit *)` is redundant if `Bash(git *)` exists
- Check across all three settings files (higher-precedence files override lower)

**Deprecated syntax** — Rules using old `:*` suffix instead of current ` *` syntax:
- `Bash(cargo build:*)` should be `Bash(cargo build *)`

**One-off rules** — Very specific commands unlikely to recur:
- Commands for tools/languages not used in this project (check for Makefile, pyproject.toml, etc.)
- System queries like `Bash(brew list)` that were one-time lookups
- Commands that Claude should use dedicated tools for (e.g., `Bash(grep *)` → use Grep tool)

**Missing rules** — Commonly-approved commands that should be permanent rules:
- Look at permission history for commands approved 3+ times
- Especially look for `cd /path && ...` patterns — if frequent, suggest `Bash(cd *)`
- Common dev commands: `tail`, `find`, `mkdir`, `cat`

**Scope suggestions** — Rules that should move between settings files:
- Project-universal patterns (cargo, npm, git, etc.) belong in `.claude/settings.json`
- Personal/machine-specific patterns (absolute paths, personal tools) belong in `.claude/settings.local.json`
- Cross-project patterns belong in `~/.claude/settings.json`

### Step 4: Present Findings

Present a summary report organized by category. For each finding, show:
- The current rule
- The issue
- The suggested action (remove, update, move, add)

Ask the user to confirm before making any changes. Offer options:
- Apply all suggestions
- Apply by category
- Cherry-pick individual changes

### Step 5: Apply Approved Changes

After user confirmation:
1. Update the settings files with approved changes
2. Validate that all modified files are valid JSON
3. Show a before/after summary of rule counts

### Output Format

```
## Permission Settings Analysis

### Stale Rules (N found)
- `Bash(./old-path/binary *)` — path doesn't exist → **remove**

### Redundant Rules (N found)
- `Bash(candle start *)` — covered by `Bash(candle *)` → **remove**

### Deprecated Syntax (N found)
- `Bash(cargo build:*)` → `Bash(cargo build *)`

### One-Off Rules (N found)
- `Bash(make)` — no Makefile in project → **remove**

### Missing Rules (N suggested)
- `Bash(cd *)` — approved N times in history → **add to settings.json**

### Scope Suggestions (N found)
- `Bash(cargo build *)` — project-universal → **move to settings.json**

---
Total: N removals, N updates, N additions, N moves
Apply changes? [all / by category / pick / skip]
```
