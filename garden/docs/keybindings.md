# Editing & keybindings

Garden's editor is a **modal vim layer** over a rope-buffered text model, plus
global Mac-style shortcuts that work in every mode.

## Vim modal editing

Buffers open in **Normal** mode; the mode shows in the status bar.

| Group | Keys | Action |
|-------|------|--------|
| Normal → Insert | `i` `a` `I` `A` `o` `O` | insert at cursor / after / first non-blank / line end / open line below / above |
| Insert → Normal | `Escape` | back to Normal |
| Motions | `h` `j` `k` `l`, `gj` `gk`, `w` `b` `e`, `0` `$`, `f` `F` `t` `T` `;` `,`, `gg` `G`, `%`, `{` `}`, arrows | move (prefix with a count, e.g. `3w`); `gj`/`gk` move down / up one *display* line under soft wrap (a wrapped visual row, keeping a sticky display column; identical to `j`/`k` when `wrap` is off); `f<c>`/`F<c>` jump to the next / previous `<c>` on the line, `t`/`T` stop one short, `;`/`,` repeat / repeat reversed; `%` jumps to the matching bracket; `{`/`}` jump to the blank line before / after the paragraph |
| Text objects | `iw` `aw`, `i(`/`a(` (+ `[ ]` `{ }` `< >`, aliases `b`≡`(` `B`≡`{`, either delimiter char works), `i"`/`a"` `i'` `` i` `` | after an operator (`diw`, `ca{`, `ya"`) or in Visual mode (`viw`, `vi(` reshapes the selection): `iw`/`aw` = the word/space/punct run under the cursor (`aw` adds the trailing — or leading — whitespace); `i(` = inside the nearest enclosing pair (multi-line), `a(` includes the delimiters; quotes are single-line (cursor inside the span, else the next pair on the line). No enclosing pair / no quote cancels the operator with no edit. (No count support; `a"` grabs only the quotes.) |
| Scroll | `zt` `zz` `zb` | reposition the viewport so the cursor's line sits at the top / center / bottom (cursor stays put) |
| Edits | `x` `dd` `dw` `D` `cc` `C` `r<c>` `J` | delete char / line / word / to-EOL, change line / to-EOL, replace char, join lines (`3J`, and Visual-mode `J`) |
| Yank/paste | `yy` `yw` `p` `P` | yank line / word, paste after / before — yanks/deletes also copy to the system clipboard, and `p`/`P` paste from it when it changed |
| Undo/redo | `u` / `Ctrl+R` | undo / redo, count-aware (`3u`) — one insert session is one undo step (Cmd+Z / Shift+Cmd+Z also work, in every mode) |
| Visual | `v` / `V` (charwise / linewise), then motions | select; `o` swaps the selection ends, `v`/`V` toggle the mode |
| Visual ops | `d` `x` `y` `c`, `>` `<`, `~` `u` `U` | delete / yank / change, indent / dedent, toggle-case / lower / upper (one undo step each) |
| Search | `/pat` `?pat` `n` `N` `*` `#` | search forward / backward (plain text, smartcase, wraps), repeat / repeat reversed, word under cursor forward / backward (`*`/`#` whole-word) — matches highlighted; `:noh` clears them. Works inside a focused **panel region** too (the `garden diff` / `garden pr` unified stream and after column): the prompt is the same one, and the pattern searches that region's buffer |
| Command line | `:e <file>` `:E` `:Git` `:Diff [--stat] [base]` `:Review [base]` `:Review2 [base]` `:PR [n]` `:w` `:q` `:wq` `:wa` `:wqa` `:noh` | open file / directory browser (alias `:Explore`; `-` does the same) / git history browser / the diff review — an editable unified diff (edit the diff to edit the change, `^S` writes the files back) plus an editable before/after split and a read-only stat view, all in the `garden-diff` client; `:Review`/`:Review2`/`:ReviewSplit` are aliases of `:Diff`, and `:PR [n]` scopes it to a GitHub PR (description, conversation, inline comments); its **commits** view lists the review's commits — click one to scope the diff to it, right-click for more / write / close (quit from the last, vim-style; `:x` = `:wq`) / write+close / write all / write all+quit / clear highlights |
| Substitute | `:s/pat/rep/[flags]` `:%s/...` `:1,5s/...` | replace on the current line / whole buffer / a line range (`N,M`, `.` = cursor, `$` = last); plain text; flags `g` (all on a line), `i` (ignore case, `I` forces exact); empty pattern reuses the last search; one undo step |
| Report | `:report <text>` | file a bug / feature report with the last 5 minutes of session events attached (stored in `~/.garden/state`) |
| Inspect | `:State` | toggle the Petal-IDE live-state inspector overlay on panel panes (every value the last frame bound, by name, + frame count) |
| History | `Ctrl+[` `Ctrl+]` / `:back` `:forward` | step back / forward through the focused panel's browser-style history — its in-script `navigate(...)` steps first, then to the previously visited `.ptl` screen; a no-op at the ends (`:fwd` = `:forward`). *(In a plain TUI terminal `Ctrl+[` is indistinguishable from Escape, so use `:back` there; the keys work in the windowed frontend and kitty-protocol terminals.)* |

Counts combine with motions and operators (`2dd`, `d3w`, `3x`). Mouse selection
still works, and typing in Insert mode replaces a selection as one undo step.

**Auto-indent** (vim's `autoindent`): Enter in Insert mode and `o`/`O` carry the
current line's leading whitespace onto the new line, cursor after the indent;
Enter inside the indent carries only the part left of the cursor. Plain
copy-indent only — no language awareness — and pasting never re-indents.

## Global keybindings (every mode)

| Key | Action |
|-----|--------|
| Cmd+S | save focused pane |
| Cmd+Shift+S | save all panes (dirty panes with a file path) |
| Cmd+Z / Shift+Cmd+Z | undo / redo |
| Cmd+A / Ctrl+A | select all |
| Cmd+C / Ctrl+C | copy the selection to the system clipboard |
| Cmd+X / Ctrl+X | cut the selection to the system clipboard (one undo step) |
| Cmd+V / Ctrl+V | paste the clipboard, replacing any selection (one undo step) |
| Cmd+W | close the window (each Garden window is its own process, so this exits — like Cmd+Q) |
| Cmd+Q / Ctrl+Q | quit |
| Cmd+P / Ctrl+P | fuzzy file finder (type to filter; ↑/↓ or Ctrl+P/Ctrl+N to move; Enter opens in the focused pane; Esc cancels) |
| Ctrl+W then `h` `j` `k` `l` | move focus to the pane left / down / up / right (the direction key may hold Ctrl) |
| Ctrl+W then `w` | cycle focus to the next pane |
| Ctrl+W then `o` | expand the focused pane to fill the window ("only") |
| Ctrl+W then `s` / `v` | split the focused pane — stacked (`s`) or side by side (`v`) |
| Ctrl+W then `c` / `q` | close the focused pane (`c` refuses the last one; `q` quits from it, like `:q`) |
| click / click-drag | focus pane + place cursor / select text |
| double-click / triple-click | select word (or whitespace/punctuation run) / whole line incl. newline — dragging extends word-/line-wise |
| shift+click | extend selection |
| scroll wheel | scroll the hovered pane (vertical and horizontal) |

The Ctrl clipboard/quit/select-all shortcuts are global Mac-style bindings: they
work in every vim mode and override vim's own Ctrl meanings for those keys (so
there is no Ctrl+V block-select or Ctrl+A increment). Ctrl+R (redo) and other
Ctrl chords still reach the vim layer.

## macOS menu bar (windowed frontend)

Every menu item routes through the same code path as its keyboard shortcut or
ex command, so the two never drift. Beyond mirrors of the bindings above, the
menus add these accelerators (macOS windowed only):

| Menu item | Keys | Action |
|-----------|------|--------|
| File ▸ Open Folder… | Shift+Cmd+O | pick a folder → directory browser (like `:E` on it) |
| Edit ▸ Find… | Cmd+F | open the `/` search prompt |
| Edit ▸ Find Next / Previous | Cmd+G / Shift+Cmd+G | repeat the last search (vim's `n` / `N`, any mode) |
| Go ▸ Go to File… | Cmd+P | fuzzy file finder |
| Go ▸ Back / Forward | Cmd+[ / Cmd+] | focused panel's history (like `Ctrl+[` / `Ctrl+]`) |
| Go ▸ Browse File's Directory | — | `:E` |
| Git ▸ Show Log / Diff Working Tree / Diff Stat / Review Changes | — | `:Git` / `:Diff` / `:Diff --stat` / `:Review` |
| View ▸ Toggle Soft Wrap / Line Numbers / State Inspector | — | `:set wrap`·`:set nowrap` / line-number gutter (per pane, persisted) / `:State` |
| Window ▸ Split Pane Down / Right | — / Cmd+\ | `Ctrl+W s` / `Ctrl+W v` |
| Window ▸ Next Pane, Close Pane, Close Other Panes | — | `Ctrl+W w` / `Ctrl+W c` / `Ctrl+W o` |
