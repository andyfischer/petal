//! Lightweight vi/vim emulation: Normal / Insert / Visual modes and the common
//! normal-mode commands, driven one key at a time against an [`EditorView`].
//!
//! The state machine is deliberately pure — it only touches the view's public
//! editing API — so it can be exercised entirely in unit tests without a window.

use garden_core::Point;

use crate::clipboard::Clipboard;
use crate::editor_view::{EditorView, Move, ScrollAlign};
use crate::search;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    /// Charwise visual (`v`): the selection spans exact characters.
    Visual,
    /// Linewise visual (`V`): the selection always covers whole lines.
    VisualLine,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "VISUAL LINE",
        }
    }

    /// In Normal and Visual modes the caret sits *on* a cell (block cursor);
    /// in Insert it sits *between* cells (bar cursor).
    pub fn is_block_cursor(self) -> bool {
        !matches!(self, Mode::Insert)
    }

    /// Either visual mode (charwise `v` or linewise `V`).
    pub fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine)
    }
}

/// Keys the vim layer understands, decoupled from the windowing toolkit.
#[derive(Clone, Copy, Debug)]
pub enum Key {
    Char(char),
    /// A character pressed with Ctrl held (`Ctrl('r')` = redo). Modeled here
    /// rather than intercepted in `app.rs` because Ctrl chords are mode- and
    /// count-sensitive (`3<C-r>`), and that state lives in [`VimState`].
    Ctrl(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

/// Pending normal-mode parser state plus the yank register.
#[derive(Default)]
pub struct VimState {
    pub mode: Mode,
    /// Accumulated numeric prefix (`3` in `3dd`).
    count: Option<usize>,
    /// Pending operator awaiting a motion (`d` or `c`; `y` in the extras).
    operator: Option<char>,
    /// Saw a leading `g`, waiting for the second key (`gg`).
    g_pending: bool,
    /// Saw a leading `z`, waiting for the scroll target (`zz`/`zt`/`zb`).
    z_pending: bool,
    /// `r` was pressed; the next character replaces the one(s) under the cursor.
    replace_pending: bool,
    /// Last yanked/deleted text and whether it was line-oriented.
    register: String,
    register_linewise: bool,
    /// Last search pattern (`/`, `?`, `*`, `#`); repeated by `n`/`N`. Per-pane,
    /// like the register.
    pub(crate) last_search: Option<String>,
    /// Direction of the last search (`true` = forward); `n` follows it,
    /// `N` reverses it.
    pub(crate) last_search_forward: bool,
    /// Whether the last search matches whole words only (vim's `\<pat\>`):
    /// `true` after `*`/`#`, `false` after `/`/`?`/`:s`. `n`/`N` and the
    /// viewport highlights follow it.
    pub(crate) last_search_word: bool,
    /// Whether match highlights are shown. `:noh` clears it; the next
    /// search or `n`/`N` re-arms it.
    pub(crate) search_hl: bool,
    /// `f`/`F`/`t`/`T` was pressed; the next character is the find target.
    /// `(till, forward)` — `till` is `t`/`T`, `forward` is `f`/`t`.
    find_pending: Option<(bool, bool)>,
    /// The last `f`/`F`/`t`/`T` as `(char, till, forward)`; repeated by
    /// `;`/`,`. Like `last_search`, it survives `clear_pending`.
    last_find: Option<(char, bool, bool)>,
    /// `i`/`a` was pressed after an operator (or in Visual mode); the next key
    /// names the text object (`diw`, `ca(`, `vi"`). `Some(true)` = around
    /// (`a`), `Some(false)` = inner (`i`).
    object_pending: Option<bool>,
}

impl VimState {
    fn clear_pending(&mut self) {
        self.count = None;
        self.operator = None;
        self.g_pending = false;
        self.z_pending = false;
        self.replace_pending = false;
        self.find_pending = None;
        self.object_pending = None;
    }

    /// A snapshot of the mid-command "pending" state — keystrokes buffered but
    /// not yet resolved into an action. Exposed through the debug server so a
    /// test harness can tell a genuinely idle editor apart from one that only
    /// *looks* idle: the [`Mode`] label reads `NORMAL` even while a leftover
    /// count, a dangling operator, or a `g`/`f`/text-object prefix is buffered,
    /// so a later command resolves against that stale state and appears to
    /// "fail". [`VimPending::is_clean`] is the "ready for a fresh command"
    /// check; a single `Escape` always restores it (see [`clear_pending`]).
    pub fn pending(&self) -> VimPending {
        VimPending {
            count: self.count,
            operator: self.operator,
            g_pending: self.g_pending,
            z_pending: self.z_pending,
            replace_pending: self.replace_pending,
            find_pending: self.find_pending,
            object_pending: self.object_pending,
        }
    }
}

/// The accumulated-but-unresolved half of [`VimState`] — see
/// [`VimState::pending`]. All fields empty/`false` means the editor is at a
/// clean command boundary.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VimPending {
    /// Numeric prefix typed so far (`3` in `3dd`).
    pub count: Option<usize>,
    /// Operator awaiting a motion (`d`/`c`/`y`).
    pub operator: Option<char>,
    /// A leading `g` awaiting its second key (`gg`, `gj`).
    pub g_pending: bool,
    /// A leading `z` awaiting its scroll target (`zz`/`zt`/`zb`).
    pub z_pending: bool,
    /// `r` was pressed; the next key replaces the char under the cursor.
    pub replace_pending: bool,
    /// A pending `f`/`F`/`t`/`T` as `(till, forward)`.
    pub find_pending: Option<(bool, bool)>,
    /// A pending text-object kind: `Some(true)` = around (`a`), `Some(false)`
    /// = inner (`i`).
    pub object_pending: Option<bool>,
}

impl VimPending {
    /// True when nothing is buffered — the editor is at a command boundary and
    /// ready to accept a fresh normal-mode command.
    pub fn is_clean(&self) -> bool {
        self.count.is_none()
            && self.operator.is_none()
            && !self.g_pending
            && !self.z_pending
            && !self.replace_pending
            && self.find_pending.is_none()
            && self.object_pending.is_none()
    }
}

/// What the caller should do after a key is handled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    /// `:` was pressed in Normal mode — open the command line.
    OpenCommandLine,
    /// `/` or `?` was pressed in Normal mode — open the search prompt.
    OpenSearch {
        forward: bool,
    },
    /// `-` was pressed in Normal mode — open the directory browser on the
    /// focused file's directory (vim-vinegar style; vim's `-` line motion is
    /// repurposed for this).
    OpenDirectoryBrowser,
}

/// Feed one key to the vim state machine for `view`. Yanks and deletes write
/// through to `clipboard`; `p`/`P` read it back (see [`paste`]).
pub fn handle(
    view: &mut EditorView,
    key: Key,
    visible_lines: usize,
    visible_cols: usize,
    clipboard: &mut dyn Clipboard,
) -> Action {
    match view.vim.mode {
        Mode::Insert => handle_insert(view, key, visible_lines, visible_cols),
        Mode::Normal | Mode::Visual | Mode::VisualLine => {
            // Bracket the command in an undo group so a change operator's delete
            // and the insert-session typing that follows collapse into one undo
            // step (vim: one `c…` command, and one insert session, is one `u`).
            // If the command doesn't leave us in Insert it behaves like a plain
            // single-command transaction; the group is closed right away.
            view.buffer.begin_undo_group();
            let action = handle_normal(view, key, visible_lines, visible_cols, clipboard);
            if view.vim.mode != Mode::Insert {
                view.buffer.end_undo_run();
            }
            action
        }
    }
}

fn handle_insert(
    view: &mut EditorView,
    key: Key,
    visible_lines: usize,
    visible_cols: usize,
) -> Action {
    match key {
        Key::Escape => {
            view.vim.mode = Mode::Normal;
            // One insert session is one undo step: close the coalescing run
            // so the next insert can't merge into it (`u` after `iab<Esc>ac`
            // must undo only the 'c').
            view.buffer.end_undo_run();
            // Leaving insert nudges the caret back onto the last typed char,
            // matching vim.
            view.cursor = view.step_backward_in_line(view.cursor);
            view.clamp_cursor_normal();
        }
        Key::Char(c) => view.insert(&c.to_string()),
        // Ctrl chords never insert text (vim's insert-mode Ctrl commands,
        // like Ctrl+R register paste, are not implemented).
        Key::Ctrl(_) => {}
        Key::Enter => view.insert_newline(),
        Key::Tab => view.insert("    "),
        Key::Backspace => view.backspace(),
        Key::Delete => view.delete_forward(),
        Key::Left => view.move_cursor(Move::Left, visible_lines, false),
        Key::Right => view.move_cursor(Move::Right, visible_lines, false),
        // Up/Down and Home follow visual rows under soft wrap; with wrap off
        // they reuse the plain logical-line mover. The caret may rest past the
        // last character in Insert mode, so the wrap-aware path skips the
        // Normal-mode clamp.
        Key::Up if view.wrap => view.move_display_row(true, visible_cols, false),
        Key::Down if view.wrap => view.move_display_row(false, visible_cols, false),
        Key::Up => view.move_cursor(Move::Up, visible_lines, false),
        Key::Down => view.move_cursor(Move::Down, visible_lines, false),
        Key::Home if view.wrap => {
            let (s, _) = view.cursor_visual_row_range(visible_cols);
            view.cursor = Point::new(view.cursor.line, s);
            view.anchor = None;
            view.desired_col = None;
            view.display_desired_col = None;
        }
        Key::Home => view.move_cursor(Move::LineStart, visible_lines, false),
        // End goes to the end of the *logical* line: on a wrapped continuation
        // row the caret at the visual-row break is ambiguous, so the append
        // point is the line end (the useful Insert-mode target).
        Key::End => view.move_cursor(Move::LineEnd, visible_lines, false),
        Key::PageUp => view.move_cursor(Move::PageUp, visible_lines, false),
        Key::PageDown => view.move_cursor(Move::PageDown, visible_lines, false),
    }
    Action::None
}

fn handle_normal(
    view: &mut EditorView,
    key: Key,
    visible_lines: usize,
    visible_cols: usize,
    clipboard: &mut dyn Clipboard,
) -> Action {
    // A non-character key while waiting for a find target or a text-object
    // kind cancels the pending command and is swallowed (vim: `f<Esc>` /
    // `f<Left>` / `di<Esc>` do nothing).
    if (view.vim.find_pending.is_some() || view.vim.object_pending.is_some())
        && !matches!(key, Key::Char(_))
    {
        view.vim.clear_pending();
        return Action::None;
    }

    let c = match key {
        Key::Char(c) => c,
        // Ctrl+R redoes, count-aware, in Normal mode only (vim's visual-mode
        // Ctrl chords are not implemented).
        Key::Ctrl('r') if view.vim.mode == Mode::Normal => {
            undo_redo(view, true);
            return Action::None;
        }
        // Ctrl+D / Ctrl+U scroll the file down / up half a viewport (vim's
        // half-page motions). In Visual mode they extend the selection like any
        // other motion; the enclosing `ensure_cursor_visible` does the scroll.
        Key::Ctrl('d') => {
            let extend = view.vim.mode.is_visual();
            view.move_cursor(Move::HalfPageDown, visible_lines, extend);
            return Action::None;
        }
        Key::Ctrl('u') => {
            let extend = view.vim.mode.is_visual();
            view.move_cursor(Move::HalfPageUp, visible_lines, extend);
            return Action::None;
        }
        Key::Ctrl(_) => {
            view.vim.clear_pending();
            return Action::None;
        }
        Key::Escape => {
            view.vim.clear_pending();
            view.vim.mode = Mode::Normal;
            view.anchor = None;
            return Action::None;
        }
        // Arrow / navigation keys act as their motion equivalents. Up/Down and
        // Home/End are wrap-aware for plain cursor movement (they follow visual
        // rows under soft wrap); with an operator pending they keep the
        // logical-line semantics of `dj`/`dk`/`d0`/`d$`.
        Key::Left => return motion_key(view, Motion::Left, clipboard),
        Key::Right => return motion_key(view, Motion::Right, clipboard),
        Key::Up => return arrow_vertical(view, true, visible_cols, clipboard),
        Key::Down => return arrow_vertical(view, false, visible_cols, clipboard),
        Key::Home => return arrow_line_edge(view, false, visible_cols, clipboard),
        Key::End => return arrow_line_edge(view, true, visible_cols, clipboard),
        Key::Backspace => return motion_key(view, Motion::Left, clipboard),
        Key::Enter => return motion_key(view, Motion::Down, clipboard),
        _ => return Action::None,
    };

    // `r<char>` replaces the character(s) under the cursor.
    if view.vim.replace_pending {
        view.vim.replace_pending = false;
        replace_char(view, c);
        return Action::None;
    }

    // `f`/`F`/`t`/`T` was pressed: this key is the character to find. Handled
    // before counts, the visual block, and the operator gate so any character
    // (a digit, `d`, …) can be a find target.
    if let Some((till, forward)) = view.vim.find_pending.take() {
        view.vim.last_find = Some((c, till, forward));
        return run_find(view, c, till, forward, false, clipboard);
    }

    // `i`/`a` was pressed after an operator or in Visual mode: this key names
    // the text object (`w`, a bracket, a quote). Handled alongside the find
    // target, before counts, so digits and operator letters can't intercept it.
    if let Some(around) = view.vim.object_pending.take() {
        return run_text_object(view, around, c, clipboard);
    }

    // Numeric prefix. A leading `0` is the line-start motion, not a count.
    if c.is_ascii_digit() && !(c == '0' && view.vim.count.is_none()) {
        let d = c as usize - '0' as usize;
        view.vim.count = Some(view.vim.count.unwrap_or(0) * 10 + d);
        return Action::None;
    }

    // Second key of a `g` chord.
    if view.vim.g_pending {
        view.vim.g_pending = false;
        match c {
            'g' => {
                let last = view.buffer.line_count().saturating_sub(1);
                let target = view
                    .vim
                    .count
                    .take()
                    .map(|n| (n - 1).min(last))
                    .unwrap_or(0);
                // `dgg`/`ygg`/`cgg` operate linewise over `[cursor, target]`; a
                // bare `gg` just moves. gg/G honor `startofline` for the yank
                // cursor.
                match view.vim.operator.take() {
                    Some(op) => {
                        let (l0, l1) = (view.cursor.line.min(target), view.cursor.line.max(target));
                        operate_lines(view, op, l0, l1, clipboard, true);
                    }
                    None => goto_line(view, target),
                }
            }
            // `gj`/`gk`: move down/up by one *visual* (soft-wrapped) row rather
            // than a whole logical line. With wrap off (or a line that fits) they
            // reduce to `j`/`k`.
            'j' => display_line_motion(view, false, visible_cols, clipboard),
            'k' => display_line_motion(view, true, visible_cols, clipboard),
            _ => {
                view.vim.count = None;
                view.vim.operator = None;
            }
        }
        return Action::None;
    }

    // Second key of a `z` scroll chord (`zz`/`zt`/`zb`): reposition the
    // viewport around the cursor without moving it.
    if view.vim.z_pending {
        view.vim.z_pending = false;
        view.vim.count = None;
        let align = match c {
            'z' => Some(ScrollAlign::Center),
            't' => Some(ScrollAlign::Top),
            'b' => Some(ScrollAlign::Bottom),
            _ => None,
        };
        if let Some(align) = align {
            view.scroll_cursor_to(align, visible_lines);
        }
        return Action::None;
    }

    // In Visual mode the operators act on the selection immediately and the
    // visual-mode keys (`v`/`V`/`o`) reshape it; everything else (motions,
    // counts) falls through to extend the selection.
    if view.vim.mode.is_visual() {
        match c {
            'd' | 'x' => {
                visual_delete(view, false, clipboard);
                return Action::None;
            }
            // `s` is a visual-mode synonym for `c`: change the selection.
            'c' | 's' => {
                visual_delete(view, true, clipboard);
                return Action::None;
            }
            'y' => {
                visual_yank(view, clipboard);
                return Action::None;
            }
            '>' => {
                visual_indent(view, false);
                return Action::None;
            }
            '<' => {
                visual_indent(view, true);
                return Action::None;
            }
            '~' => {
                visual_case(view, CaseOp::Toggle);
                return Action::None;
            }
            'u' => {
                visual_case(view, CaseOp::Lower);
                return Action::None;
            }
            'U' => {
                visual_case(view, CaseOp::Upper);
                return Action::None;
            }
            // `J`: join every line the selection spans onto its first line.
            'J' => {
                let (lo, hi) = visual_line_span(view);
                join_lines(view, lo, (hi - lo + 1).max(2));
                let cursor = view.cursor;
                finish_visual_edit(view, cursor);
                return Action::None;
            }
            'o' => {
                // Swap the fixed and moving ends so a motion now grows the
                // other side of the selection.
                if let Some(anchor) = view.anchor {
                    view.anchor = Some(view.cursor);
                    view.cursor = anchor;
                    view.desired_col = None;
                }
                view.vim.clear_pending();
                return Action::None;
            }
            // `i`/`a` await a text-object key that reshapes the selection
            // (`viw`, `va(`); they never enter Insert from Visual mode.
            'i' | 'a' => {
                view.vim.object_pending = Some(c == 'a');
                return Action::None;
            }
            // `v` / `V` switch between charwise and linewise, or exit when
            // pressed in their own mode.
            'v' => {
                if view.vim.mode == Mode::Visual {
                    exit_visual(view);
                } else {
                    view.vim.mode = Mode::Visual;
                    view.vim.clear_pending();
                }
                return Action::None;
            }
            'V' => {
                if view.vim.mode == Mode::VisualLine {
                    exit_visual(view);
                } else {
                    view.vim.mode = Mode::VisualLine;
                    view.vim.clear_pending();
                }
                return Action::None;
            }
            _ => {}
        }
    }

    // With an operator pending (`d`/`c`/`y`), only a motion, a `g`-chord, or the
    // operator's own letter (the linewise double, `cc`/`dd`/`yy`) continues it;
    // counts were already consumed above. Any other key cancels the operator and
    // is swallowed — vim treats `cC`, `cx`, `cr`, `ci`, `dc`, … as no-ops rather
    // than running the second key as its own command.
    if let Some(op) = view.vim.operator {
        // `i`/`a` start a text object (`diw`, `ca(`) rather than entering
        // Insert; the next key names the object kind.
        if c == 'i' || c == 'a' {
            view.vim.object_pending = Some(c == 'a');
            return Action::None;
        }
        let continues = "hljkwbe0^$%GgfFtT;,{}".contains(c) || c == op;
        if !continues {
            view.vim.clear_pending();
            return Action::None;
        }
    }

    match c {
        ':' => return Action::OpenCommandLine,
        '/' => return Action::OpenSearch { forward: true },
        '?' => return Action::OpenSearch { forward: false },

        // `-` opens the directory browser (vim-vinegar). Only in Normal mode;
        // in Visual it falls through and clears pending state.
        '-' if view.vim.mode == Mode::Normal => {
            view.vim.clear_pending();
            return Action::OpenDirectoryBrowser;
        }

        // Search repeats and word search.
        'n' => repeat_search(view, false),
        'N' => repeat_search(view, true),
        '*' => star_search(view, true),
        '#' => star_search(view, false),

        'v' => {
            view.vim.mode = Mode::Visual;
            view.anchor = Some(view.cursor);
            view.vim.clear_pending();
        }
        'V' => {
            view.vim.mode = Mode::VisualLine;
            view.anchor = Some(view.cursor);
            view.vim.clear_pending();
        }

        // Enter Insert mode.
        'i' => enter_insert(view, InsertAt::Cursor),
        'a' => enter_insert(view, InsertAt::After),
        'I' => enter_insert(view, InsertAt::FirstNonBlank),
        'A' => enter_insert(view, InsertAt::LineEnd),
        'o' => {
            view.open_below();
            view.vim.mode = Mode::Insert;
            view.vim.clear_pending();
        }
        'O' => {
            view.open_above();
            view.vim.mode = Mode::Insert;
            view.vim.clear_pending();
        }

        // Motions.
        'h' => return motion_key(view, Motion::Left, clipboard),
        'l' => return motion_key(view, Motion::Right, clipboard),
        'j' => return motion_key(view, Motion::Down, clipboard),
        'k' => return motion_key(view, Motion::Up, clipboard),
        'w' => return motion_key(view, Motion::WordForward, clipboard),
        'b' => return motion_key(view, Motion::WordBackward, clipboard),
        'e' => return motion_key(view, Motion::WordEnd, clipboard),
        '0' => return motion_key(view, Motion::LineStart, clipboard),
        '^' => return motion_key(view, Motion::FirstNonBlank, clipboard),
        '$' => return motion_key(view, Motion::LineEnd, clipboard),
        '%' => return motion_key(view, Motion::MatchPair, clipboard),
        '}' => return motion_key(view, Motion::ParaForward, clipboard),
        '{' => return motion_key(view, Motion::ParaBackward, clipboard),
        // Char finds: wait for the target character (`f`/`t` forward,
        // `F`/`T` backward; `t`/`T` stop one short of it).
        'f' | 'F' | 't' | 'T' => {
            view.vim.find_pending = Some((matches!(c, 't' | 'T'), c.is_ascii_lowercase()));
            return Action::None;
        }
        ';' => return repeat_find(view, false, clipboard),
        ',' => return repeat_find(view, true, clipboard),
        'G' => {
            let last = view.buffer.line_count().saturating_sub(1);
            let target = view
                .vim
                .count
                .take()
                .map(|n| (n - 1).min(last))
                .unwrap_or(last);
            // `dG`/`yG`/`cG` operate linewise to the target line; bare `G` moves.
            match view.vim.operator.take() {
                Some(op) => {
                    let (l0, l1) = (view.cursor.line.min(target), view.cursor.line.max(target));
                    operate_lines(view, op, l0, l1, clipboard, true);
                }
                None => goto_line(view, target),
            }
        }
        'g' => {
            view.vim.g_pending = true;
            return Action::None;
        }
        // `z` scroll chord: the next key (`z`/`t`/`b`) recenters the viewport.
        'z' => {
            view.vim.z_pending = true;
            return Action::None;
        }

        // Single-key edits.
        'x' => {
            let n = view.vim.count.take().unwrap_or(1);
            delete_chars(view, n, clipboard);
            view.vim.operator = None;
        }
        // `s`: substitute — delete the char(s) under the cursor and insert in
        // their place (vim's `cl`). `Ns` substitutes `count` characters.
        's' => {
            let n = view.vim.count.take().unwrap_or(1);
            substitute_chars(view, n, clipboard);
        }
        // `S`: substitute the whole line — clear `count` lines to one empty
        // line and enter Insert (vim's `cc`).
        'S' => {
            let n = view.vim.count.take().unwrap_or(1);
            change_lines(view, view.cursor.line, n);
        }
        'D' => {
            // `D` is `d$`; `ND` is `dN$` (delete to the end of the line N-1 rows
            // down). A count that can't reach that far is a no-op, like `dj` at
            // the bottom.
            let n = view.vim.count.take().unwrap_or(1);
            let last = view.buffer.line_count().saturating_sub(1);
            if n == 1 {
                delete_to_line_end(view, clipboard);
            } else if view.cursor.line + n - 1 <= last {
                apply_operator(view, 'd', Motion::LineEnd, n, clipboard);
            }
            view.vim.clear_pending();
        }
        'C' => {
            delete_to_line_end(view, clipboard);
            view.vim.mode = Mode::Insert;
            view.vim.clear_pending();
        }
        'r' => {
            view.vim.replace_pending = true;
            return Action::None;
        }
        // `~` / `N~`: toggle the case of the character(s) under the cursor and
        // advance. The Visual-mode `~` (whole selection) is handled above.
        '~' => {
            let n = view.vim.count.take().unwrap_or(1);
            toggle_case_chars(view, n);
            view.vim.clear_pending();
        }
        // `J`: join the line(s) below onto this one. `J`/`2J` join one
        // following line; `NJ` joins `N-1`.
        'J' => {
            collapse_selection_in_normal(view);
            let n = view.vim.count.take().unwrap_or(1).max(2);
            join_lines(view, view.cursor.line, n);
            view.vim.clear_pending();
        }
        'u' => undo_redo(view, false),
        'p' => {
            let n = view.vim.count.take().unwrap_or(1);
            paste(view, true, n, clipboard);
            view.vim.clear_pending();
        }
        'P' => {
            let n = view.vim.count.take().unwrap_or(1);
            paste(view, false, n, clipboard);
            view.vim.clear_pending();
        }

        // Operators (await a motion, or double for the whole line).
        'd' | 'c' | 'y' => {
            if view.vim.operator == Some(c) {
                let n = view.vim.count.take().unwrap_or(1);
                let l0 = view.cursor.line;
                let l1 = (l0 + n - 1).min(view.buffer.line_count().saturating_sub(1));
                // `Ndd` behaves like `d(N-1)j`: with a count that can't extend
                // down at all (last line, N>1) the motion fails and vim cancels
                // the whole edit. A bare `dd`/`cc`/`yy` (N==1) still acts.
                if n == 1 || l1 != l0 {
                    // `yy` keeps its column; `dd`/`cc` land on the first non-blank.
                    operate_lines(view, c, l0, l1, clipboard, false);
                }
                view.vim.operator = None;
            } else {
                view.vim.operator = Some(c);
            }
        }

        _ => view.vim.clear_pending(),
    }

    Action::None
}

#[derive(Clone, Copy)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBackward,
    WordEnd,
    LineStart,
    /// `^`: the first non-blank character of the line (`0` is the true start).
    FirstNonBlank,
    LineEnd,
    /// `%`: jump to the bracket matching the first one at/after the cursor on
    /// the current line. A no-op (target == cursor) when there is no bracket
    /// on the line or its match is missing.
    MatchPair,
    /// `}`: forward to the blank line after the current paragraph (exclusive).
    ParaForward,
    /// `{`: backward to the blank line before the current paragraph (exclusive).
    ParaBackward,
}

impl Motion {
    fn is_linewise(self) -> bool {
        matches!(self, Motion::Up | Motion::Down)
    }

    fn is_inclusive(self) -> bool {
        matches!(self, Motion::WordEnd | Motion::MatchPair)
    }
}

/// Selections only live in Visual mode: any Normal-mode cursor movement
/// collapses a selection left behind by the mouse, so it can't silently
/// extend and be swallowed by the next paste or insert.
pub(crate) fn collapse_selection_in_normal(view: &mut EditorView) {
    if view.vim.mode == Mode::Normal {
        view.anchor = None;
    }
}

/// Run a motion either as cursor movement or as the target of a pending
/// operator, then reset the parser.
fn motion_key(view: &mut EditorView, motion: Motion, clipboard: &mut dyn Clipboard) -> Action {
    collapse_selection_in_normal(view);
    let count = view.vim.count.take().unwrap_or(1);
    match view.vim.operator.take() {
        Some(op) => apply_operator(view, op, motion, count, clipboard),
        None => match motion {
            // Vertical motions move by whole lines at the desired column,
            // remembered across short lines (`desired_col`).
            Motion::Up | Motion::Down => {
                let desired = view.desired_col.unwrap_or(view.cursor.col);
                let line = match motion {
                    Motion::Up => view.cursor.line.saturating_sub(count),
                    _ => view.cursor.line + count,
                };
                view.cursor = view.buffer.clamp(Point::new(line, desired));
                view.clamp_cursor_normal();
                view.desired_col = Some(desired);
            }
            _ => {
                view.cursor = motion_target(view, motion, count);
                view.clamp_cursor_normal();
                // `$` pins the desired column to end-of-line so subsequent
                // j/k ride the line ends, as in vim.
                view.desired_col = matches!(motion, Motion::LineEnd).then_some(usize::MAX);
            }
        },
    }
    view.vim.g_pending = false;
    Action::None
}

/// `gj` / `gk`: move the cursor down (`up == false`) or up by `count` *visual*
/// (soft-wrapped) rows, keeping a sticky **display** column across a run of such
/// moves — wrapped rows begin at different char columns, so the on-screen x, not
/// the char col, is what's preserved (mirrors `desired_col` for `j`/`k`, but in
/// display space). With wrap off, or a line short enough to occupy one visual
/// row, this reduces to plain `j`/`k`.
///
/// With an operator pending the edit follows the visual geometry: within one
/// soft-wrapped logical line (`dgj` on a wrapped line) it is **charwise** over
/// the exclusive display span `[cursor, target)` — deleting from the cursor to
/// the same display column one visual row away. When the motion crosses a
/// logical line boundary it is **linewise** over the spanned lines, like
/// `dj`/`dk` (and identical to them with wrap off, where every line is one
/// visual row).
fn display_line_motion(
    view: &mut EditorView,
    up: bool,
    visible_cols: usize,
    clipboard: &mut dyn Clipboard,
) {
    let cols = visible_cols.max(1);
    let count = view.vim.count.take().unwrap_or(1);
    let desired = view
        .display_desired_col
        .unwrap_or_else(|| view.cursor_display_col(cols));
    let target = view.display_line_target(up, count, cols, desired);

    if let Some(op) = view.vim.operator.take() {
        if target == view.cursor {
            return; // pinned at the top/bottom visual row: operator aborts
        }
        if target.line == view.cursor.line {
            // Same logical line, different visual row: charwise over the
            // exclusive display span.
            let (a, b) = (view.cursor.min(target), view.cursor.max(target));
            char_op(view, op, a, b, clipboard);
            return;
        }
        let (l0, l1) = (
            view.cursor.line.min(target.line),
            view.cursor.line.max(target.line),
        );
        operate_lines(view, op, l0, l1, clipboard, false);
        return;
    }

    // Pure movement (Normal) or Visual-mode extension: a lingering mouse
    // selection collapses in Normal, the Visual anchor is kept.
    collapse_selection_in_normal(view);
    view.cursor = target;
    view.clamp_cursor_normal();
    // clamp_cursor_normal cleared the sticky column; re-establish it so a run
    // of gj/gk holds the target display column through short rows.
    view.display_desired_col = Some(desired);
}

/// Arrow Up/Down in Normal/Visual mode: wrap-aware cursor movement (one visual
/// row) for plain navigation, but linewise like `dk`/`dj` when an operator is
/// pending — arrows keep vim's `j`/`k` operator semantics, only `gj`/`gk` are
/// charwise display motions.
fn arrow_vertical(
    view: &mut EditorView,
    up: bool,
    visible_cols: usize,
    clipboard: &mut dyn Clipboard,
) -> Action {
    if view.vim.operator.is_some() {
        return motion_key(view, if up { Motion::Up } else { Motion::Down }, clipboard);
    }
    // With no operator this is exactly the gj/gk movement path: wrap-aware step
    // by one visual row, sticky display column, Visual anchor kept.
    display_line_motion(view, up, visible_cols, clipboard);
    view.vim.g_pending = false;
    Action::None
}

/// Home/End in Normal/Visual mode: move to the start / end of the cursor's
/// current *visual* row (wrap-aware; the whole logical line with wrap off). With
/// an operator pending it defers to the logical-line edge motion (`d0`/`d$`).
fn arrow_line_edge(
    view: &mut EditorView,
    end: bool,
    visible_cols: usize,
    clipboard: &mut dyn Clipboard,
) -> Action {
    if view.vim.operator.is_some() {
        let motion = if end {
            Motion::LineEnd
        } else {
            Motion::LineStart
        };
        return motion_key(view, motion, clipboard);
    }
    view.vim.count = None;
    collapse_selection_in_normal(view);
    if view.vim.mode.is_visual() && view.anchor.is_none() {
        view.anchor = Some(view.cursor);
    }
    let line = view.cursor.line;
    let (s, e) = view.cursor_visual_row_range(visible_cols);
    // On a wrapped continuation row `e` is the first column of the next visual
    // row, so End lands one short of it to stay on this row; the final row's `e`
    // is end-of-line, which clamp pulls back onto the last character.
    let is_last_row = e >= view.buffer.line_len(line);
    view.cursor = if end {
        let col = if is_last_row { e } else { e.saturating_sub(1) };
        view.buffer.clamp(Point::new(line, col))
    } else {
        Point::new(line, s)
    };
    view.clamp_cursor_normal();
    // Mirror `$`: pin the sticky columns to end-of-row so subsequent vertical
    // moves ride the row ends.
    view.desired_col = end.then_some(usize::MAX);
    view.display_desired_col = end.then_some(usize::MAX);
    view.vim.g_pending = false;
    Action::None
}

/// Perform a linewise operator over the inclusive line range `[l0, l1]`.
///
/// Cursor placement follows vim: delete lands on the first non-blank of the line
/// that slides up (vim's `startofline`); change clears the lines and enters
/// Insert; yank lands on the first line of the range, at its first non-blank for
/// `startofline` motions (`gg`/`G`) or the preserved column otherwise (`yy`/`yj`).
fn operate_lines(
    view: &mut EditorView,
    op: char,
    l0: usize,
    l1: usize,
    clipboard: &mut dyn Clipboard,
    startofline: bool,
) {
    let n = l1 - l0 + 1;
    match op {
        'c' => change_lines(view, l0, n),
        'y' => {
            yank_lines(view, l0, n, clipboard);
            // A downward (or same-line) yank leaves the caret where it is; only
            // an upward yank moves it to the first line of the range — at the
            // first non-blank for `startofline` motions (`ygg`), else the kept
            // column (`yk`).
            if l0 < view.cursor.line {
                let col = if startofline {
                    first_non_blank(view, l0)
                } else {
                    view.cursor.col
                };
                view.cursor = view.buffer.clamp(Point::new(l0, col));
                view.clamp_cursor_normal();
            }
        }
        _ => {
            yank_lines(view, l0, n, clipboard);
            view.delete_lines(l0, n);
            let line = l0.min(view.buffer.line_count().saturating_sub(1));
            view.cursor = Point::new(line, first_non_blank(view, line));
            view.clamp_cursor_normal();
        }
    }
}

fn apply_operator(
    view: &mut EditorView,
    op: char,
    motion: Motion,
    count: usize,
    clipboard: &mut dyn Clipboard,
) {
    if motion.is_linewise() {
        let target = motion_target(view, motion, count);
        // A relative linewise motion (`j`/`k`) that can't move at all aborts the
        // whole operator, as in vim — `dk` at the top / `dj` at the bottom are
        // no-ops rather than deleting the current line.
        if target.line == view.cursor.line {
            return;
        }
        let (l0, l1) = (
            view.cursor.line.min(target.line),
            view.cursor.line.max(target.line),
        );
        operate_lines(view, op, l0, l1, clipboard, false);
        return;
    }

    // vim's `cw`/`cW` special case: with the cursor on a non-blank, `cw` changes
    // to the *end of the current word* (like `ce`) rather than over the trailing
    // whitespace (`dw`) — "a word does not include the following white space".
    // On whitespace it falls through to the normal `w` motion (like `dw`). The
    // end differs subtly from `ce`: on the last char of a word `cw` changes only
    // that char, where `ce` would jump to the next word's end (see change_word_end).
    if op == 'c' && matches!(motion, Motion::WordForward) && char_class(view, view.cursor) != 0 {
        let end = change_word_end(view, view.cursor, count);
        let a = view.cursor;
        let b = view.step_forward(end).unwrap_or(end);
        set_register(view, clipboard, view.buffer.text_range(a, b), false);
        view.cursor = view.erase(a, b);
        view.vim.mode = Mode::Insert;
        return;
    }

    let mut target = motion_target(view, motion, count);
    // `%` with no matching bracket doesn't move; the operator is then a no-op
    // (without this guard the inclusive-motion bump below would delete one char).
    if matches!(motion, Motion::MatchPair) && target == view.cursor {
        return;
    }
    // A counted `N$` that can't move down at all (fewer than N lines remain)
    // fails like `Nj`, cancelling the operator — `c2$`/`d2$` on the last line is
    // a no-op. When it *can* move down it clamps to the last line as usual.
    if matches!(motion, Motion::LineEnd) && count > 1 && target.line == view.cursor.line {
        return;
    }
    // vim's special case: an exclusive motion (notably `w`) that lands in
    // column 0 of a later line backs up to the end of the previous line, so
    // `dw` on the last word of a line empties it instead of joining the next.
    if matches!(motion, Motion::WordForward) && target.line > view.cursor.line && target.col == 0 {
        let prev = target.line - 1;
        target = Point::new(prev, view.buffer.line_len(prev));
    }
    let a = view.cursor.min(target);
    let mut b = view.cursor.max(target);
    if motion.is_inclusive() {
        b = view.step_forward(b).unwrap_or(b);
    }
    // A multi-line `N$` delete also consumes the last line's trailing newline,
    // so the spanned lines are removed rather than leaving an empty leading line
    // (`d2$` joins away the intervening rows). Change (`c2$`) keeps the newline
    // so the caret is left on an empty line to type on; single-line `d$` stays
    // charwise regardless.
    if matches!(motion, Motion::LineEnd) && target.line > view.cursor.line && op != 'c' {
        b = view.step_forward(b).unwrap_or(b);
    }
    if a == b {
        // A zero-width change still opens Insert for the *clamping* charwise
        // motions (`c0`/`ch`/`cl`/`c$`): vim's `0`/`h`/`l`/`$` stop at the line
        // boundary rather than failing, so `c` there starts an empty insert.
        // `cw` (WordForward) reaches this only on an empty line — vim's `cw`
        // never joins the next line, so it too opens an empty insert. Backward
        // word motions (`cb`/`cB`) and `%` instead fail when they can't move,
        // cancelling the operator with no insert.
        if op == 'c'
            && matches!(
                motion,
                Motion::Left
                    | Motion::Right
                    | Motion::LineStart
                    | Motion::FirstNonBlank
                    | Motion::LineEnd
                    | Motion::WordForward
            )
        {
            view.vim.mode = Mode::Insert;
        }
        return;
    }
    char_op(view, op, a, b, clipboard);
}

/// Yank, change, or delete the charwise span `[a, b)`: write the register,
/// then apply the edit and place the cursor (`c` opens Insert).
fn char_op(view: &mut EditorView, op: char, a: Point, b: Point, clipboard: &mut dyn Clipboard) {
    set_register(view, clipboard, view.buffer.text_range(a, b), false);
    match op {
        'y' => {
            view.cursor = a;
            view.clamp_cursor_normal();
        }
        'c' => {
            view.cursor = view.erase(a, b);
            view.vim.mode = Mode::Insert;
        }
        _ => {
            view.cursor = view.erase(a, b);
            view.clamp_cursor_normal();
        }
    }
}

/// `u` / Ctrl+R: undo or redo `count` transactions. The buffer restores an
/// insert-style cursor position; pull it back onto a real character like
/// every other normal-mode edit.
fn undo_redo(view: &mut EditorView, redo: bool) {
    let count = view.vim.count.take().unwrap_or(1);
    for _ in 0..count {
        if redo {
            view.redo_vim();
        } else {
            view.undo();
        }
    }
    view.clamp_cursor_normal();
    view.vim.clear_pending();
}

fn motion_target(view: &EditorView, motion: Motion, count: usize) -> Point {
    // `N$` goes to the end of the line `N-1` rows down (vim's count on `$`),
    // rather than repeating end-of-line on the current row.
    if let Motion::LineEnd = motion {
        let last = view.buffer.line_count().saturating_sub(1);
        let line = (view.cursor.line + count.saturating_sub(1)).min(last);
        return Point::new(line, view.buffer.line_len(line));
    }
    let mut p = view.cursor;
    for _ in 0..count {
        p = match motion {
            Motion::Left => Point::new(p.line, p.col.saturating_sub(1)),
            Motion::Right => Point::new(p.line, (p.col + 1).min(view.buffer.line_len(p.line))),
            Motion::Up => view
                .buffer
                .clamp(Point::new(p.line.saturating_sub(1), p.col)),
            Motion::Down => view.buffer.clamp(Point::new(p.line + 1, p.col)),
            Motion::WordForward => word_forward(view, p),
            Motion::WordBackward => word_backward(view, p),
            Motion::WordEnd => word_end(view, p),
            Motion::LineStart => Point::new(p.line, 0),
            Motion::FirstNonBlank => Point::new(p.line, first_non_blank(view, p.line)),
            Motion::LineEnd => Point::new(p.line, view.buffer.line_len(p.line)),
            // `%` ignores any count and stays put when there is no match.
            Motion::MatchPair => match_pair(view, p).unwrap_or(p),
            Motion::ParaForward => para_forward(view, p),
            Motion::ParaBackward => para_backward(view, p),
        };
    }
    p
}

/// The three bracket pairs `%` matches, as `(open, close)`.
const BRACKET_PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// Find the position of the bracket matching the first one at or after `from`
/// on `from`'s line (vim's `%`). Returns `None` if the line has no bracket at
/// or after the cursor, or the bracket is unbalanced.
fn match_pair(view: &EditorView, from: Point) -> Option<Point> {
    let line: Vec<char> = view.buffer.line(from.line).chars().collect();
    // Seek rightward on the line for the first bracket character.
    let (col, &(open, close), opener) = (from.col..line.len()).find_map(|col| {
        let c = line[col];
        BRACKET_PAIRS.iter().find_map(|pair| match c {
            _ if c == pair.0 => Some((col, pair, true)),
            _ if c == pair.1 => Some((col, pair, false)),
            _ => None,
        })
    })?;
    scan_for_match(view, Point::new(from.line, col), open, close, opener)
}

/// Scan from a bracket at `start` for its partner, tracking nesting depth.
/// `forward` (an opener) scans toward the end of the buffer for `close`;
/// otherwise it scans backward toward the start for `open`.
fn scan_for_match(
    view: &EditorView,
    start: Point,
    open: char,
    close: char,
    forward: bool,
) -> Option<Point> {
    let mut depth = 0i32;
    let mut line = start.line;
    let mut chars: Vec<char> = view.buffer.line(line).chars().collect();
    let mut col = start.col as isize;
    loop {
        let c = chars[col as usize];
        if c == open {
            depth += if forward { 1 } else { -1 };
        } else if c == close {
            depth += if forward { -1 } else { 1 };
        }
        if depth == 0 {
            return Some(Point::new(line, col as usize));
        }
        col += if forward { 1 } else { -1 };
        // Step to the next/previous non-empty line when we run off this one.
        if forward && col as usize >= chars.len() {
            loop {
                line += 1;
                if line >= view.buffer.line_count() {
                    return None;
                }
                chars = view.buffer.line(line).chars().collect();
                if !chars.is_empty() {
                    break;
                }
            }
            col = 0;
        } else if !forward && col < 0 {
            loop {
                if line == 0 {
                    return None;
                }
                line -= 1;
                chars = view.buffer.line(line).chars().collect();
                if !chars.is_empty() {
                    break;
                }
            }
            col = chars.len() as isize - 1;
        }
    }
}

// --- text objects (`iw`/`aw`, bracket pairs, quotes) --------------------------

/// Execute a text object named by `kind` after `i`/`a` (`around`), either
/// feeding the pending operator (`diw`, `ca(`) or reshaping the Visual
/// selection (`viw`). An unresolvable object (no enclosing pair, no quote on
/// the line, unknown kind) cancels the operator with no edit; in Visual mode
/// it leaves the selection untouched.
fn run_text_object(
    view: &mut EditorView,
    around: bool,
    kind: char,
    clipboard: &mut dyn Clipboard,
) -> Action {
    // A count widens the object (`d2aw`, `d2i(`): word objects take that many
    // runs/words, bracket pairs step out that many nesting levels. Quote objects
    // ignore it (as vim does).
    let count = view.vim.count.take().unwrap_or(1).max(1);
    let span = text_object_span(view, around, kind, count);

    if view.vim.mode.is_visual() {
        view.vim.clear_pending();
        // Unresolvable or empty object: keep the current selection (vim errors
        // out and leaves the selection; an empty `i(` selection can't exist in
        // our inclusive-selection model, so it too is left unchanged).
        if let Some((a, b)) = span {
            if a < b {
                // Charwise reshape even from linewise Visual: anchor at the
                // object's start, cursor on its last character.
                view.vim.mode = Mode::Visual;
                view.anchor = Some(a);
                view.cursor = view.step_backward(b).unwrap_or(a);
                view.desired_col = None;
            }
        }
        return Action::None;
    }

    let Some(op) = view.vim.operator.take() else {
        view.vim.clear_pending();
        return Action::None;
    };
    view.vim.clear_pending();
    let Some((a, b)) = span else {
        return Action::None; // object not found: operator cancelled, no edit
    };
    if a == b {
        // Empty object (`di(` on `()`): nothing to delete or yank, but `c`
        // still opens an insert between the delimiters.
        if op == 'c' {
            view.cursor = a;
            view.vim.mode = Mode::Insert;
        }
        return Action::None;
    }
    char_op(view, op, a, b, clipboard);
    Action::None
}

/// The exclusive-end char span `[a, b)` of a text object, or `None` when it
/// can't be resolved at the cursor. `kind` accepts both characters of each
/// bracket pair plus vim's `b`/`B` aliases. `count` widens the object (see
/// [`run_text_object`]).
fn text_object_span(
    view: &EditorView,
    around: bool,
    kind: char,
    count: usize,
) -> Option<(Point, Point)> {
    match kind {
        'w' => Some(word_object_span(view, around, count)),
        '(' | ')' | 'b' => pair_object_span(view, around, '(', ')', count),
        '[' | ']' => pair_object_span(view, around, '[', ']', count),
        '{' | '}' | 'B' => pair_object_span(view, around, '{', '}', count),
        '<' | '>' => pair_object_span(view, around, '<', '>', count),
        // Quote objects ignore the count (vim's behavior is quirky enough that
        // dropping it is the least-surprising choice).
        '"' | '\'' | '`' => quote_object_span(view, around, kind),
        _ => None,
    }
}

/// `iw` / `aw`: the same-class run under the cursor (word chars, whitespace,
/// or punctuation — [`EditorView::word_range_at`]). `aw` also takes the
/// trailing whitespace run; with none, the leading whitespace instead. On
/// whitespace, `aw` takes the blanks plus the following word (vim), falling
/// back to the preceding word at end of line.
///
/// `count > 1` widens the object: `iw` grows to `count` consecutive runs
/// (word/whitespace alternating, as vim counts each), and `aw` grows to
/// `count` words, each with its trailing whitespace (`d2aw`). Single-line; an
/// empty line yields an empty span.
fn word_object_span(view: &EditorView, around: bool, count: usize) -> (Point, Point) {
    let (mut start, mut end) = view.word_range_at(view.cursor);
    let line = view.cursor.line;
    let len = view.buffer.line_len(line);
    let class = |col: usize| char_class(view, Point::new(line, col));
    if !around {
        // `iw` / `Niw`: extend forward over whole same-class runs, `count` runs
        // total (the run under the cursor plus `count - 1` following runs).
        for _ in 1..count {
            if end.col >= len {
                break;
            }
            let cls = class(end.col);
            while end.col < len && class(end.col) == cls {
                end.col += 1;
            }
        }
        return (start, end);
    }
    let extend_end_while = |end: &mut Point, cls: u8| {
        let mut grew = false;
        while end.col < len && class(end.col) == cls {
            end.col += 1;
            grew = true;
        }
        grew
    };
    let extend_start_while = |start: &mut Point, cls: u8| {
        while start.col > 0 && class(start.col - 1) == cls {
            start.col -= 1;
        }
    };
    if char_class(view, view.cursor) == 0 {
        // On whitespace: the blanks plus the following run; at end of line,
        // the preceding run instead.
        if end.col < len {
            let cls = class(end.col);
            extend_end_while(&mut end, cls);
        } else if start.col > 0 {
            let cls = class(start.col - 1);
            extend_start_while(&mut start, cls);
        }
    } else if !extend_end_while(&mut end, 0) {
        // No trailing whitespace: take the leading whitespace instead.
        extend_start_while(&mut start, 0);
    }
    // `Naw`: each further count consumes another word and its trailing
    // whitespace (any leading whitespace between the previous word and this
    // one is already inside the span). Stops at end of line.
    for _ in 1..count {
        if end.col >= len {
            break;
        }
        while end.col < len && class(end.col) == 0 {
            end.col += 1;
        }
        while end.col < len && class(end.col) != 0 {
            end.col += 1;
        }
        while end.col < len && class(end.col) == 0 {
            end.col += 1;
        }
    }
    (start, end)
}

/// `i(`/`a(` and friends: the contents (or full extent) of the nearest
/// enclosing bracket pair. `count > 1` steps out that many nesting levels
/// (`d2i(` targets the pair one level out). `None` with no enclosing pair (or
/// fewer than `count` levels of nesting).
fn pair_object_span(
    view: &EditorView,
    around: bool,
    open: char,
    close: char,
    count: usize,
) -> Option<(Point, Point)> {
    let (mut opener, mut closer) = enclosing_pair_at(view, view.cursor, open, close)?;
    // Each extra count re-runs the enclosing-pair scan from just outside the
    // current opener, climbing one bracket level per step.
    for _ in 1..count {
        let outside = view.step_backward(opener)?;
        (opener, closer) = enclosing_pair_at(view, outside, open, close)?;
    }
    if around {
        Some((opener, view.step_forward(closer).unwrap_or(closer)))
    } else {
        Some((view.step_forward(opener).unwrap_or(opener), closer))
    }
}

/// Find the nearest `open`…`close` pair enclosing `cursor` (multi-line),
/// returning the delimiter positions. A cursor sitting ON a delimiter counts
/// as inside that pair (vim: `di(` with the cursor on `(` empties it).
fn enclosing_pair_at(
    view: &EditorView,
    cursor: Point,
    open: char,
    close: char,
) -> Option<(Point, Point)> {
    let under = view.buffer.line(cursor.line).chars().nth(cursor.col);
    if under == Some(close) {
        let opener = scan_for_match(view, cursor, open, close, false)?;
        return Some((opener, cursor));
    }
    let opener = if under == Some(open) {
        cursor
    } else {
        unmatched_open_before(view, cursor, open, close)?
    };
    let closer = scan_for_match(view, opener, open, close, true)?;
    Some((opener, closer))
}

/// Scan backward from just before `from` for an `open` bracket with no
/// matching `close` in between (the opener of the pair enclosing `from`).
fn unmatched_open_before(view: &EditorView, from: Point, open: char, close: char) -> Option<Point> {
    let mut line = from.line;
    let mut chars: Vec<char> = view.buffer.line(line).chars().collect();
    let mut col = from.col as isize - 1;
    let mut depth = 0usize;
    loop {
        while col < 0 {
            if line == 0 {
                return None;
            }
            line -= 1;
            chars = view.buffer.line(line).chars().collect();
            col = chars.len() as isize - 1;
        }
        let c = chars[col as usize];
        if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                return Some(Point::new(line, col as usize));
            }
            depth -= 1;
        }
        col -= 1;
    }
}

/// `i"`/`a"` (and `'`, `` ` ``): the quoted span on the cursor's line, pairing
/// quotes left-to-right from the line start as vim does. The pair containing
/// the cursor wins; otherwise the first pair starting after it (vim's
/// forward-seek). A quote preceded by an odd number of backslashes is escaped
/// and does not delimit (`"a\"b"` is one string). `a"` also takes the trailing
/// whitespace after the closing quote (or, with none, the leading whitespace
/// before the opener), matching vim. Single-line only; `None` with no usable
/// pair.
fn quote_object_span(view: &EditorView, around: bool, quote: char) -> Option<(Point, Point)> {
    let line = view.cursor.line;
    let chars: Vec<char> = view.buffer.line(line).chars().collect();
    let positions: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|&(i, &c)| c == quote && !is_escaped(&chars, i))
        .map(|(i, _)| i)
        .collect();
    let pair = positions
        .chunks_exact(2)
        .find(|pair| pair[1] >= view.cursor.col)?;
    if !around {
        return Some((Point::new(line, pair[0] + 1), Point::new(line, pair[1])));
    }
    // `a"`: include the quotes, then the trailing whitespace after the closer;
    // if there is none, the leading whitespace before the opener instead.
    let mut start = pair[0];
    let mut end = pair[1] + 1;
    let mut grew = false;
    while end < chars.len() && chars[end].is_whitespace() {
        end += 1;
        grew = true;
    }
    if !grew {
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
    }
    Some((Point::new(line, start), Point::new(line, end)))
}

/// Whether the char at `i` is backslash-escaped: preceded by an odd number of
/// consecutive backslashes.
fn is_escaped(chars: &[char], i: usize) -> bool {
    let mut backslashes = 0;
    let mut j = i;
    while j > 0 && chars[j - 1] == '\\' {
        backslashes += 1;
        j -= 1;
    }
    backslashes % 2 == 1
}

// --- char find (`f`/`F`/`t`/`T`, repeated by `;`/`,`) -------------------------

/// `;` / `,`: repeat the last `f`/`F`/`t`/`T` — `,` with the direction
/// reversed (`f`↔`F`, `t`↔`T`). A no-op when nothing was found yet.
fn repeat_find(view: &mut EditorView, reverse: bool, clipboard: &mut dyn Clipboard) -> Action {
    let Some((ch, till, forward)) = view.vim.last_find else {
        view.vim.clear_pending();
        return Action::None;
    };
    run_find(view, ch, till, forward != reverse, true, clipboard)
}

/// Execute a char find: move the cursor (extending the selection in Visual
/// mode) or feed a pending operator. `f`/`t` are inclusive operator motions,
/// `F`/`T` exclusive. A failed find (no `count`-th occurrence on the line)
/// moves nothing and cancels the operator.
fn run_find(
    view: &mut EditorView,
    ch: char,
    till: bool,
    forward: bool,
    is_repeat: bool,
    clipboard: &mut dyn Clipboard,
) -> Action {
    collapse_selection_in_normal(view);
    let count = view.vim.count.take().unwrap_or(1);
    let op = view.vim.operator.take();
    let mut target = find_char_target(view, ch, till, forward, count, false);
    // vim's `;`-after-`t` special case: when repeating a till would leave the
    // cursor where it is (the target char is adjacent), search one char
    // further instead of stalling.
    if is_repeat && till && target == Some(view.cursor) {
        target = find_char_target(view, ch, till, forward, count, true);
    }
    let Some(target) = target else {
        return Action::None; // failed: cursor stays, any operator is dropped
    };
    match op {
        Some(op) => operate_char_span(view, op, target, forward, clipboard),
        None => {
            view.cursor = target;
            view.clamp_cursor_normal();
            view.desired_col = None;
        }
    }
    Action::None
}

/// Resolve a char find on the cursor's line (finds never cross lines): the
/// position of the `count`-th occurrence of `ch` strictly after (before) the
/// cursor, adjusted one column short for `t` / one past for `T`.
/// `skip_adjacent` starts the scan one character further, for the stalled-`t`
/// repeat. `None` when there aren't `count` occurrences.
fn find_char_target(
    view: &EditorView,
    ch: char,
    till: bool,
    forward: bool,
    count: usize,
    skip_adjacent: bool,
) -> Option<Point> {
    let line: Vec<char> = view.buffer.line(view.cursor.line).chars().collect();
    let col = view.cursor.col;
    let mut remaining = count;
    let mut hit = |i: &usize| {
        line[*i] == ch && {
            remaining -= 1;
            remaining == 0
        }
    };
    let found = if forward {
        ((col + 1 + skip_adjacent as usize).min(line.len())..line.len()).find(&mut hit)
    } else {
        (0..col.saturating_sub(skip_adjacent as usize).min(line.len()))
            .rev()
            .find(&mut hit)
    }?;
    let target = match (till, forward) {
        (true, true) => found - 1,
        (true, false) => found + 1,
        _ => found,
    };
    Some(Point::new(view.cursor.line, target))
}

/// Apply an operator over the charwise span from the cursor to an
/// already-resolved motion target (the tail of [`apply_operator`], minus its
/// motion-specific special cases). An empty exclusive span (`dTx` with the
/// target adjacent) cancels the operator.
fn operate_char_span(
    view: &mut EditorView,
    op: char,
    target: Point,
    inclusive: bool,
    clipboard: &mut dyn Clipboard,
) {
    let a = view.cursor.min(target);
    let mut b = view.cursor.max(target);
    if inclusive {
        b = view.step_forward(b).unwrap_or(b);
    }
    if a == b {
        return;
    }
    char_op(view, op, a, b, clipboard);
}

// --- search ------------------------------------------------------------------

/// `n` / `N`: repeat the last search `count` times, in its direction
/// (`reverse` flips it). Re-arms the match highlights (vim's `n` after
/// `:noh`). A no-op when nothing was searched yet.
fn repeat_search(view: &mut EditorView, reverse: bool) {
    collapse_selection_in_normal(view);
    let count = view.vim.count.take().unwrap_or(1);
    view.vim.clear_pending(); // search keys are not operator motions (no `dn`)
    let Some(pattern) = view.vim.last_search.clone() else {
        return;
    };
    let forward = view.vim.last_search_forward != reverse;
    let whole_word = view.vim.last_search_word;
    view.vim.search_hl = true;
    for _ in 0..count {
        match search::find_next(&view.buffer, view.cursor, &pattern, forward, whole_word) {
            Some(p) => view.cursor = p,
            None => break,
        }
    }
    view.desired_col = None;
}

/// `*` (`forward`) / `#` (backward): search for the word under the cursor,
/// matching whole words only (vim's `\<word\>` boundaries). Simplified vs
/// vim: the cursor must already be on a word character (vim would scan ahead).
fn star_search(view: &mut EditorView, forward: bool) {
    collapse_selection_in_normal(view);
    view.vim.clear_pending();
    if char_class(view, view.cursor) != 1 {
        return; // not on a word character
    }
    let (start, end) = view.word_range_at(view.cursor);
    let word = view.buffer.text_range(start, end);
    view.vim.last_search = Some(word.clone());
    view.vim.last_search_forward = forward;
    view.vim.last_search_word = true;
    view.vim.search_hl = true;
    if let Some(p) = search::find_next(&view.buffer, view.cursor, &word, forward, true) {
        view.cursor = p;
        view.desired_col = None;
    }
}

/// Move the cursor to `line` (clamped to the buffer), landing on its first
/// non-blank column — `gg`/`G`, and the `:N` ex command via
/// [`crate::app::App::run_command`].
pub(crate) fn goto_line(view: &mut EditorView, line: usize) {
    collapse_selection_in_normal(view);
    let line = line.min(view.buffer.line_count().saturating_sub(1));
    view.cursor = Point::new(line, first_non_blank(view, line));
    view.desired_col = None;
    view.vim.count = None;
}

// --- insert entry -----------------------------------------------------------

enum InsertAt {
    Cursor,
    After,
    FirstNonBlank,
    LineEnd,
}

fn enter_insert(view: &mut EditorView, at: InsertAt) {
    let line = view.cursor.line;
    let len = view.buffer.line_len(line);
    view.cursor.col = match at {
        InsertAt::Cursor => view.cursor.col,
        InsertAt::After => (view.cursor.col + 1).min(len),
        InsertAt::FirstNonBlank => first_non_blank(view, line),
        InsertAt::LineEnd => len,
    };
    view.vim.mode = Mode::Insert;
    view.anchor = None;
    view.vim.clear_pending();
}

// --- edits ------------------------------------------------------------------

fn delete_chars(view: &mut EditorView, count: usize, clipboard: &mut dyn Clipboard) {
    let line = view.cursor.line;
    let len = view.buffer.line_len(line);
    if view.cursor.col >= len {
        return;
    }
    let end = Point::new(line, (view.cursor.col + count).min(len));
    set_register(
        view,
        clipboard,
        view.buffer.text_range(view.cursor, end),
        false,
    );
    view.cursor = view.erase(view.cursor, end);
    view.clamp_cursor_normal();
}

/// `s` / `Ns`: delete `count` characters under the cursor (yanking them like
/// `x`) and enter Insert mode where they were. Unlike [`delete_chars`] the
/// caret keeps its raw post-delete column — Insert mode wants the bar-cursor
/// position, not the block-cursor clamp. On an empty line or past the last
/// character there is nothing to delete, but we still enter Insert.
fn substitute_chars(view: &mut EditorView, count: usize, clipboard: &mut dyn Clipboard) {
    let line = view.cursor.line;
    let len = view.buffer.line_len(line);
    if view.cursor.col < len {
        let end = Point::new(line, (view.cursor.col + count).min(len));
        set_register(
            view,
            clipboard,
            view.buffer.text_range(view.cursor, end),
            false,
        );
        view.cursor = view.erase(view.cursor, end);
    }
    view.vim.mode = Mode::Insert;
    view.anchor = None;
    view.vim.clear_pending();
}

fn delete_to_line_end(view: &mut EditorView, clipboard: &mut dyn Clipboard) {
    let line = view.cursor.line;
    let len = view.buffer.line_len(line);
    if view.cursor.col < len {
        let end = Point::new(line, len);
        set_register(
            view,
            clipboard,
            view.buffer.text_range(view.cursor, end),
            false,
        );
        view.cursor = view.erase(view.cursor, end);
    }
    view.clamp_cursor_normal();
}

/// `J`: merge `lines` consecutive lines (the line at `start` plus `lines - 1`
/// below it) into a single line, as one undo step. Each joined line's leading
/// whitespace is dropped and a single space inserted at the seam — except when
/// the running text already ends in whitespace, the joined line is blank, or it
/// begins with `)` (vim's rules). The caret lands on the last seam.
fn join_lines(view: &mut EditorView, start: usize, lines: usize) {
    let last = view.buffer.line_count().saturating_sub(1);
    let following = lines.saturating_sub(1).min(last - start);
    if following == 0 {
        return; // nothing below to join
    }
    let mut result = view.buffer.line(start);
    let mut seam = view.buffer.line_len(start);
    for k in 1..=following {
        let next = view.buffer.line(start + k);
        let trimmed = next.trim_start();
        let no_space = trimmed.is_empty()
            || result.chars().last().is_none_or(|c| c.is_whitespace())
            || trimmed.starts_with(')');
        seam = result.chars().count();
        if !no_space {
            result.push(' ');
        }
        result.push_str(trimmed);
    }
    let end = start + following;
    view.edit(
        Point::new(start, 0),
        Point::new(end, view.buffer.line_len(end)),
        &result,
    );
    view.cursor = Point::new(start, seam);
    view.clamp_cursor_normal();
}

/// `~` / `N~`: toggle the case of `count` characters starting under the cursor,
/// then advance past them (vim's normal-mode tilde). Clamped to the end of the
/// line; on an empty line or past the last character it does nothing.
fn toggle_case_chars(view: &mut EditorView, count: usize) {
    let line = view.cursor.line;
    let len = view.buffer.line_len(line);
    if view.cursor.col >= len {
        return;
    }
    let end = Point::new(line, (view.cursor.col + count).min(len));
    let recased: String = view
        .buffer
        .text_range(view.cursor, end)
        .chars()
        .map(|c| apply_case(c, CaseOp::Toggle))
        .collect();
    view.edit(view.cursor, end, &recased);
    view.buffer.end_undo_run();
    view.cursor = end;
    view.clamp_cursor_normal();
}

fn replace_char(view: &mut EditorView, ch: char) {
    let n = view.vim.count.take().unwrap_or(1);
    let line = view.cursor.line;
    let len = view.buffer.line_len(line);
    if view.cursor.col >= len {
        return;
    }
    let end = (view.cursor.col + n).min(len);
    let replacement: String = std::iter::repeat_n(ch, end - view.cursor.col).collect();
    view.buffer
        .replace(view.cursor, Point::new(line, end), &replacement);
    view.cursor = Point::new(line, end.saturating_sub(1));
    view.clamp_cursor_normal();
}

// --- register, yank and paste -----------------------------------------------

/// Update the register and write the text through to the clipboard, so vim
/// yanks/deletes are visible to Cmd+V, other panes, and other applications.
fn set_register(
    view: &mut EditorView,
    clipboard: &mut dyn Clipboard,
    text: String,
    linewise: bool,
) {
    clipboard.set(&text);
    view.vim.register = text;
    view.vim.register_linewise = linewise;
}

/// Yank `count` whole lines starting at `start` into the register (without the
/// trailing newline; the line-oriented flag records that it was whole lines).
fn yank_lines(view: &mut EditorView, start: usize, count: usize, clipboard: &mut dyn Clipboard) {
    let end = (start + count).min(view.buffer.line_count());
    let text = (start..end)
        .map(|i| view.buffer.line(i))
        .collect::<Vec<_>>()
        .join("\n");
    set_register(view, clipboard, text, true);
}

/// Paste for `p`/`P`. The text comes from the clipboard when it has any
/// (falling back to the internal register otherwise, e.g. when no OS
/// pasteboard is available). Linewise semantics follow the register only
/// while the clipboard still matches what vim last wrote; text changed
/// externally is pasted characterwise.
fn paste(view: &mut EditorView, after: bool, count: usize, clipboard: &mut dyn Clipboard) {
    let (text, linewise) = match clipboard.get().filter(|t| !t.is_empty()) {
        Some(t) => {
            let linewise = view.vim.register_linewise && t == view.vim.register;
            (t, linewise)
        }
        None => (view.vim.register.clone(), view.vim.register_linewise),
    };
    if text.is_empty() || count == 0 {
        return;
    }
    if linewise {
        // `Np` inserts `count` copies of the yanked line block.
        let block = std::iter::repeat_n(text.as_str(), count)
            .collect::<Vec<_>>()
            .join("\n");
        let line = view.cursor.line;
        if after {
            let at = Point::new(line, view.buffer.line_len(line));
            view.insert_at(at, &format!("\n{block}"));
            view.cursor = Point::new(line + 1, first_non_blank(view, line + 1));
        } else {
            view.buffer
                .insert(Point::new(line, 0), &format!("{block}\n"));
            view.cursor = Point::new(line, first_non_blank(view, line));
        }
    } else {
        // `Np` inserts the yanked text `count` times, caret on the last char.
        let block = text.repeat(count);
        let len = view.buffer.line_len(view.cursor.line);
        let at = if after && len > 0 {
            Point::new(view.cursor.line, (view.cursor.col + 1).min(len))
        } else {
            view.cursor
        };
        let end = view.insert_at(at, &block);
        // Leave the caret on the last pasted character, as vim does.
        view.cursor = view.step_backward_in_line(end);
        view.clamp_cursor_normal();
    }
}

// --- visual mode ------------------------------------------------------------

/// The visual selection as an inclusive `[start, end)` char range: vim's visual
/// highlight always covers at least the character under the cursor, so even a
/// zero-width `anchor == cursor` selection (e.g. `vd`, or `vb` at column 0 where
/// the motion couldn't move) still operates on that one character.
fn visual_range(view: &EditorView) -> Option<(Point, Point)> {
    let anchor = view.anchor?;
    let (a, b) = (anchor.min(view.cursor), anchor.max(view.cursor));
    Some((a, view.step_forward(b).unwrap_or(b)))
}

/// The inclusive line range `(first, last)` covered by the visual selection.
fn visual_line_span(view: &EditorView) -> (usize, usize) {
    match view.selection() {
        Some(sel) => {
            let (a, b) = sel.ordered();
            (a.line, b.line)
        }
        None => (view.cursor.line, view.cursor.line),
    }
}

/// Delete the visual selection. With `change`, enter Insert mode afterwards.
/// Linewise (`V`) deletes whole lines and yanks them linewise so `p` re-inserts
/// them as new lines.
fn visual_delete(view: &mut EditorView, change: bool, clipboard: &mut dyn Clipboard) {
    if view.vim.mode == Mode::VisualLine {
        let (lo, hi) = visual_line_span(view);
        yank_lines(view, lo, hi - lo + 1, clipboard);
        view.anchor = None;
        if change {
            change_lines(view, lo, hi - lo + 1); // clears the lines, enters Insert
        } else {
            view.delete_lines(lo, hi - lo + 1);
            // Land on the first non-blank of the line that slides up (vim's
            // `startofline`), like `dd`.
            let line = lo.min(view.buffer.line_count().saturating_sub(1));
            view.cursor = Point::new(line, first_non_blank(view, line));
            view.vim.mode = Mode::Normal;
            view.clamp_cursor_normal();
        }
        view.vim.clear_pending();
        return;
    }
    if let Some((a, b)) = visual_range(view) {
        set_register(view, clipboard, view.buffer.text_range(a, b), false);
        view.cursor = view.erase(a, b);
    }
    view.anchor = None;
    view.vim.mode = if change { Mode::Insert } else { Mode::Normal };
    if !change {
        view.clamp_cursor_normal();
    }
    view.vim.clear_pending();
}

fn visual_yank(view: &mut EditorView, clipboard: &mut dyn Clipboard) {
    if view.vim.mode == Mode::VisualLine {
        let (lo, hi) = visual_line_span(view);
        yank_lines(view, lo, hi - lo + 1, clipboard);
        view.cursor = Point::new(lo, 0);
    } else if let Some((a, b)) = visual_range(view) {
        set_register(view, clipboard, view.buffer.text_range(a, b), false);
        view.cursor = a;
    }
    view.anchor = None;
    view.vim.mode = Mode::Normal;
    view.clamp_cursor_normal();
    view.vim.clear_pending();
}

/// Leave any visual mode, dropping the selection and pulling the caret back
/// onto a real character (block-cursor rule).
fn exit_visual(view: &mut EditorView) {
    view.vim.mode = Mode::Normal;
    view.anchor = None;
    view.clamp_cursor_normal();
    view.vim.clear_pending();
}

/// After a visual-mode edit that already mutated the buffer: drop the
/// selection, return to Normal at `cursor`, clamp, and reset the parser.
fn finish_visual_edit(view: &mut EditorView, cursor: Point) {
    view.anchor = None;
    view.vim.mode = Mode::Normal;
    view.cursor = cursor;
    view.clamp_cursor_normal();
    view.vim.clear_pending();
}

/// The full-line char range `[start, end)` of the linewise selection: the
/// first column of the first line through the end of the last line.
fn visual_line_bounds(view: &EditorView) -> (Point, Point) {
    let (lo, hi) = visual_line_span(view);
    (Point::new(lo, 0), Point::new(hi, view.buffer.line_len(hi)))
}

/// One indentation level. Tab keys also insert four spaces (see `handle_insert`).
const INDENT: &str = "    ";

/// `>` / `<` on the visually selected lines: add or remove one indent level,
/// as a single undo transaction. Blank lines are left untouched when indenting.
fn visual_indent(view: &mut EditorView, dedent: bool) {
    let (a, b) = visual_line_bounds(view);
    let reindented = view
        .buffer
        .text_range(a, b)
        .split('\n')
        .map(|line| reindent(line, dedent))
        .collect::<Vec<_>>()
        .join("\n");
    view.edit(a, b, &reindented);
    view.buffer.end_undo_run();
    // vim drops the caret on the first non-blank of the first touched line.
    let cursor = Point::new(a.line, first_non_blank(view, a.line));
    finish_visual_edit(view, cursor);
}

fn reindent(line: &str, dedent: bool) -> String {
    if dedent {
        if let Some(rest) = line.strip_prefix('\t') {
            rest.to_string()
        } else {
            // Drop up to one indent level of leading spaces.
            let spaces = line
                .chars()
                .take_while(|c| *c == ' ')
                .count()
                .min(INDENT.len());
            line[spaces..].to_string()
        }
    } else if line.is_empty() {
        String::new()
    } else {
        format!("{INDENT}{line}")
    }
}

#[derive(Clone, Copy)]
enum CaseOp {
    Lower,
    Upper,
    Toggle,
}

/// `u` / `U` / `~` on the selection: ASCII case change (keeping column counts
/// stable, like the smartcase search), as one undo transaction. Charwise (`v`)
/// touches the exact range; linewise (`V`) touches whole lines.
fn visual_case(view: &mut EditorView, op: CaseOp) {
    let (a, b) = if view.vim.mode == Mode::VisualLine {
        visual_line_bounds(view)
    } else {
        visual_range(view).unwrap_or((view.cursor, view.cursor))
    };
    let recased: String = view
        .buffer
        .text_range(a, b)
        .chars()
        .map(|c| apply_case(c, op))
        .collect();
    view.edit(a, b, &recased);
    view.buffer.end_undo_run();
    finish_visual_edit(view, a);
}

fn apply_case(c: char, op: CaseOp) -> char {
    match op {
        CaseOp::Lower => c.to_ascii_lowercase(),
        CaseOp::Upper => c.to_ascii_uppercase(),
        CaseOp::Toggle if c.is_ascii_uppercase() => c.to_ascii_lowercase(),
        CaseOp::Toggle if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
        CaseOp::Toggle => c,
    }
}

/// `cc` / `cN`: clear the target lines down to a single empty line and enter
/// Insert mode there.
fn change_lines(view: &mut EditorView, start: usize, count: usize) {
    let last = view.buffer.line_count().saturating_sub(1);
    // `NS` / `Ncc` behaves like `c(N-1)j`: on the last line a count > 1 can't
    // move down, so vim cancels the change rather than clearing the line.
    if count > 1 && start >= last {
        view.vim.clear_pending();
        return;
    }
    let end = (start + count - 1).min(last);
    let a = Point::new(start, 0);
    let b = Point::new(end, view.buffer.line_len(end));
    view.cursor = view.erase(a, b);
    view.vim.mode = Mode::Insert;
    view.vim.clear_pending();
}

// --- word motion helpers ----------------------------------------------------

/// Character class for word motions: 0 = whitespace/end-of-line, 1 = word
/// character (alphanumeric or `_`), 2 = punctuation. Also the word
/// classification behind double-click word selection
/// ([`EditorView::word_range_at`]).
pub(crate) fn char_class(view: &EditorView, p: Point) -> u8 {
    if p.col >= view.buffer.line_len(p.line) {
        return 0; // the newline between lines counts as whitespace
    }
    match view.buffer.line(p.line).chars().nth(p.col) {
        Some(ch) if ch.is_alphanumeric() || ch == '_' => 1,
        Some(ch) if ch.is_whitespace() => 0,
        Some(_) => 2,
        None => 0,
    }
}

/// Step forward while `pred` holds for the class at the current position.
fn step_while(view: &EditorView, mut p: Point, pred: impl Fn(u8) -> bool) -> Point {
    while pred(char_class(view, p)) {
        match view.step_forward(p) {
            Some(next) => p = next,
            None => break,
        }
    }
    p
}

fn word_forward(view: &EditorView, p: Point) -> Point {
    let start = char_class(view, p);
    let p = if start != 0 {
        step_while(view, p, |c| c == start)
    } else {
        p
    };
    step_while(view, p, |c| c == 0)
}

fn word_end(view: &EditorView, p: Point) -> Point {
    let mut cur = view.step_forward(p).unwrap_or(p);
    while char_class(view, cur) == 0 {
        match view.step_forward(cur) {
            Some(next) => cur = next,
            None => return cur,
        }
    }
    let cls = char_class(view, cur);
    while let Some(next) = view.step_forward(cur) {
        if char_class(view, next) == cls {
            cur = next;
        } else {
            break;
        }
    }
    cur
}

/// The end target for vim's `cw`/`cW` (cursor on a non-blank). Mirrors vim's
/// `end_word` with its `stop` flag set on the first word: the first word only
/// extends to the end of the run *currently under the cursor* — it never leaps
/// to the next word the way `e` does when already on a word's last char. Later
/// words (count > 1) behave like plain `e`. Returns the inclusive last char.
fn change_word_end(view: &EditorView, p: Point, count: usize) -> Point {
    let mut cur = p;
    for i in 0..count {
        let stop = i == 0;
        let sclass = char_class(view, cur);
        let next = match view.step_forward(cur) {
            Some(n) => n,
            None => return cur,
        };
        if char_class(view, next) == sclass && sclass != 0 {
            // Mid-word: extend to the end of the current-class run.
            cur = end_of_run(view, next, sclass);
        } else if !stop || sclass == 0 {
            // Past a word (or on whitespace): skip blanks, then to the run end.
            cur = next;
            while char_class(view, cur) == 0 {
                match view.step_forward(cur) {
                    Some(n) => cur = n,
                    None => return cur,
                }
            }
            cur = end_of_run(view, cur, char_class(view, cur));
        }
        // else (stop && on the last char of a word): stay put — cur unchanged.
    }
    cur
}

/// Advance to the last position whose class stays `cls` (vim's forward
/// `skip_chars`, landing on the run's last char rather than one past it).
fn end_of_run(view: &EditorView, mut cur: Point, cls: u8) -> Point {
    while let Some(next) = view.step_forward(cur) {
        if char_class(view, next) == cls {
            cur = next;
        } else {
            break;
        }
    }
    cur
}

fn word_backward(view: &EditorView, p: Point) -> Point {
    let mut cur = match view.step_backward(p) {
        Some(prev) => prev,
        None => return p,
    };
    while char_class(view, cur) == 0 {
        match view.step_backward(cur) {
            Some(prev) => cur = prev,
            None => return cur,
        }
    }
    let cls = char_class(view, cur);
    while let Some(prev) = view.step_backward(cur) {
        if char_class(view, prev) == cls {
            cur = prev;
        } else {
            break;
        }
    }
    cur
}

/// One step of `}`: from a blank starting line, first skip the run of blanks
/// down to the paragraph; then move to the first blank line at or after the
/// first non-blank. Only a truly empty line (length 0) is blank — a
/// whitespace-only line belongs to its paragraph, as in vim. With no blank
/// line below, land at the *end* of the last line (vim's `}` at EOF), so an
/// operator span covers the full remaining text.
fn para_forward(view: &EditorView, p: Point) -> Point {
    let line_count = view.buffer.line_count();
    let mut i = p.line + 1;
    if view.buffer.line_len(p.line) == 0 {
        while i < line_count && view.buffer.line_len(i) == 0 {
            i += 1;
        }
    }
    while i < line_count && view.buffer.line_len(i) != 0 {
        i += 1;
    }
    if i < line_count {
        Point::new(i, 0)
    } else {
        let last = line_count.saturating_sub(1);
        Point::new(last, view.buffer.line_len(last))
    }
}

/// One step of `{`: the upward mirror of [`para_forward`]. With no blank line
/// above, land at the start of the buffer (0, 0).
fn para_backward(view: &EditorView, p: Point) -> Point {
    if p.line == 0 {
        return Point::new(0, 0);
    }
    let mut i = p.line - 1;
    if view.buffer.line_len(p.line) == 0 {
        while i > 0 && view.buffer.line_len(i) == 0 {
            i -= 1;
        }
    }
    while i > 0 && view.buffer.line_len(i) != 0 {
        i -= 1;
    }
    if view.buffer.line_len(i) == 0 {
        Point::new(i, 0)
    } else {
        Point::new(0, 0)
    }
}

fn first_non_blank(view: &EditorView, line: usize) -> usize {
    view.buffer
        .line(line)
        .chars()
        .position(|ch| !ch.is_whitespace())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::InMemoryClipboard;
    use garden_core::Buffer;

    fn view(text: &str) -> EditorView {
        EditorView::from_buffer(Buffer::from_str(text))
    }

    /// A wrap width wide enough that no test line soft-wraps, so the vast
    /// majority of tests behave as if wrapping were off (each buffer line is one
    /// visual row). Narrow values are passed explicitly via [`keys_wrap`].
    const WIDE: usize = 10_000;

    /// Feed a run of character keys, sharing one clipboard across the run.
    /// (Across separate `keys` calls the clipboard starts empty again, which
    /// exercises the fall-back-to-register paste path.)
    fn keys(v: &mut EditorView, s: &str) {
        let mut clip = InMemoryClipboard::default();
        for ch in s.chars() {
            handle(v, Key::Char(ch), 50, WIDE, &mut clip);
        }
    }

    /// Feed a run of character keys at an explicit wrap width (columns per
    /// visual row), so the display-line motions (`gj`/`gk`) can be exercised
    /// against soft-wrapped lines.
    fn keys_wrap(v: &mut EditorView, s: &str, visible_cols: usize) {
        let mut clip = InMemoryClipboard::default();
        for ch in s.chars() {
            handle(v, Key::Char(ch), 50, visible_cols, &mut clip);
        }
    }

    /// Feed a run of character keys against a caller-owned clipboard.
    fn keys_clip(v: &mut EditorView, clip: &mut dyn Clipboard, s: &str) {
        for ch in s.chars() {
            handle(v, Key::Char(ch), 50, WIDE, clip);
        }
    }

    /// Feed a run of character keys with an explicit viewport height, so
    /// scroll-sensitive commands (`z`-chords) can be exercised at a small,
    /// predictable size.
    fn keys_vis(v: &mut EditorView, s: &str, visible_lines: usize) {
        let mut clip = InMemoryClipboard::default();
        for ch in s.chars() {
            handle(v, Key::Char(ch), visible_lines, WIDE, &mut clip);
        }
    }

    /// Feed one non-character key.
    fn key(v: &mut EditorView, k: Key) {
        handle(v, k, 50, WIDE, &mut InMemoryClipboard::default());
    }

    /// Feed one non-character key at an explicit wrap width, so the wrap-aware
    /// arrow / Home / End behavior can be exercised.
    fn key_wrap(v: &mut EditorView, k: Key, visible_cols: usize) {
        handle(v, k, 50, visible_cols, &mut InMemoryClipboard::default());
    }

    fn esc(v: &mut EditorView) {
        key(v, Key::Escape);
    }

    #[test]
    fn starts_in_normal_mode() {
        assert_eq!(view("hi").vim.mode, Mode::Normal);
    }

    #[test]
    fn pending_is_clean_at_command_boundary() {
        let mut v = view("hello world");
        assert!(v.vim.pending().is_clean());
        // A completed command leaves nothing buffered.
        keys(&mut v, "dw");
        assert!(v.vim.pending().is_clean());
    }

    #[test]
    fn pending_exposes_mid_command_state() {
        // A dangling operator: `d` awaiting its motion. The mode label still
        // reads NORMAL — the pending snapshot is what tells them apart.
        let mut v = view("hello world");
        keys(&mut v, "d");
        assert_eq!(v.vim.mode.label(), "NORMAL");
        let p = v.vim.pending();
        assert!(!p.is_clean());
        assert_eq!(p.operator, Some('d'));

        // A leftover count survives as pending, too.
        let mut v = view("hello world");
        keys(&mut v, "3");
        assert_eq!(v.vim.pending().count, Some(3));

        // A pending text-object kind (`di` awaiting the object char).
        let mut v = view("(hello)");
        keys(&mut v, "di");
        assert_eq!(v.vim.pending().object_pending, Some(false));
    }

    #[test]
    fn escape_restores_a_clean_pending_state() {
        // The recovery the debug-server note relies on: one Escape from any
        // half-typed command returns to a clean boundary, so the next command
        // resolves fresh rather than against stale accumulated state.
        let mut v = view("(hello) world");
        keys(&mut v, "2d");
        assert!(!v.vim.pending().is_clean());
        esc(&mut v);
        assert!(v.vim.pending().is_clean());
        // And a text object now behaves as if typed on a clean slate.
        keys(&mut v, "di(");
        assert_eq!(v.buffer.line(0), "() world");
    }

    #[test]
    fn dash_opens_the_directory_browser_in_normal_mode() {
        let mut v = view("one\ntwo");
        let action = handle(
            &mut v,
            Key::Char('-'),
            50,
            WIDE,
            &mut InMemoryClipboard::default(),
        );
        assert_eq!(action, Action::OpenDirectoryBrowser);
    }

    #[test]
    fn dash_in_visual_mode_does_not_open_the_browser() {
        let mut v = view("one\ntwo");
        keys(&mut v, "v"); // enter Visual mode
        let action = handle(
            &mut v,
            Key::Char('-'),
            50,
            WIDE,
            &mut InMemoryClipboard::default(),
        );
        assert_eq!(action, Action::None);
        assert_eq!(v.vim.mode, Mode::Visual);
    }

    #[test]
    fn typing_in_normal_mode_is_not_inserted() {
        let mut v = view("hello");
        keys(&mut v, "xyz"); // 'x' deletes, 'y'/'z' are no-ops here
        assert_eq!(v.buffer.to_string(), "ello");
    }

    #[test]
    fn i_inserts_text_then_escape_returns_to_normal() {
        let mut v = view("bc");
        keys(&mut v, "i");
        assert_eq!(v.vim.mode, Mode::Insert);
        keys(&mut v, "A"); // literal 'A' inserted in insert mode
        assert_eq!(v.buffer.to_string(), "Abc");
        esc(&mut v);
        assert_eq!(v.vim.mode, Mode::Normal);
        // Caret steps back off the just-typed char.
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn a_appends_after_cursor() {
        let mut v = view("ab");
        keys(&mut v, "a"); // insert after col 0
        keys(&mut v, "X");
        assert_eq!(v.buffer.to_string(), "aXb");
    }

    #[test]
    fn cap_a_appends_at_line_end() {
        let mut v = view("ab");
        keys(&mut v, "A!");
        assert_eq!(v.buffer.to_string(), "ab!");
    }

    #[test]
    fn cap_i_inserts_at_first_non_blank() {
        let mut v = view("   ab");
        keys(&mut v, "Ix");
        assert_eq!(v.buffer.to_string(), "   xab");
    }

    #[test]
    fn o_opens_line_below() {
        let mut v = view("one\ntwo");
        keys(&mut v, "ox");
        assert_eq!(v.buffer.to_string(), "one\nx\ntwo");
    }

    #[test]
    fn cap_o_opens_line_above() {
        let mut v = view("one\ntwo");
        keys(&mut v, "Ox");
        assert_eq!(v.buffer.to_string(), "x\none\ntwo");
    }

    // --- auto-indent: o / O and Enter in Insert mode ------------------------

    #[test]
    fn o_copies_the_current_line_indent() {
        let mut v = view("    one\ntwo");
        keys(&mut v, "ox");
        assert_eq!(v.buffer.to_string(), "    one\n    x\ntwo");
    }

    #[test]
    fn o_uses_the_full_indent_regardless_of_cursor_column() {
        let mut v = view("    one"); // cursor at col 0, inside the indent
        keys(&mut v, "ox");
        assert_eq!(v.buffer.to_string(), "    one\n    x");
    }

    #[test]
    fn o_places_the_cursor_after_the_indent() {
        let mut v = view("  one");
        keys(&mut v, "o");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.cursor, Point::new(1, 2));
    }

    #[test]
    fn cap_o_copies_the_current_line_indent() {
        let mut v = view("    one\ntwo");
        keys(&mut v, "Ox");
        assert_eq!(v.buffer.to_string(), "    x\n    one\ntwo");
    }

    #[test]
    fn cap_o_places_the_cursor_after_the_indent() {
        let mut v = view("\tone");
        keys(&mut v, "O");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.cursor, Point::new(0, 1));
        assert_eq!(v.buffer.to_string(), "\t\n\tone");
    }

    #[test]
    fn o_with_indent_undoes_in_one_step() {
        let mut v = view("    one");
        keys(&mut v, "o");
        esc(&mut v);
        v.undo();
        assert_eq!(v.buffer.to_string(), "    one");
    }

    #[test]
    fn enter_in_insert_mode_copies_the_indent() {
        let mut v = view("  ab");
        keys(&mut v, "A"); // insert at line end
        key(&mut v, Key::Enter);
        keys(&mut v, "x");
        assert_eq!(v.buffer.to_string(), "  ab\n  x");
    }

    #[test]
    fn enter_in_insert_mode_on_unindented_line_adds_no_indent() {
        let mut v = view("ab");
        keys(&mut v, "A");
        key(&mut v, Key::Enter);
        assert_eq!(v.buffer.to_string(), "ab\n");
        assert_eq!(v.cursor, Point::new(1, 0));
    }

    #[test]
    fn hjkl_move_the_cursor() {
        let mut v = view("abc\ndef");
        keys(&mut v, "ll");
        assert_eq!(v.cursor, Point::new(0, 2));
        keys(&mut v, "j");
        assert_eq!(v.cursor, Point::new(1, 2));
        keys(&mut v, "h");
        assert_eq!(v.cursor, Point::new(1, 1));
        keys(&mut v, "k");
        assert_eq!(v.cursor, Point::new(0, 1));
    }

    #[test]
    fn j_remembers_column_through_short_line() {
        let mut v = view("long line\nab\nlong line");
        keys(&mut v, "8l"); // col 8
        keys(&mut v, "j");
        assert_eq!(v.cursor, Point::new(1, 1)); // clamped onto 'b'
        keys(&mut v, "j");
        assert_eq!(v.cursor, Point::new(2, 8)); // desired column restored
    }

    #[test]
    fn k_remembers_column_through_short_line() {
        let mut v = view("long line\nab\nlong line");
        keys(&mut v, "G8l"); // last line, col 8
        keys(&mut v, "k");
        assert_eq!(v.cursor, Point::new(1, 1));
        keys(&mut v, "k");
        assert_eq!(v.cursor, Point::new(0, 8));
    }

    #[test]
    fn count_j_keeps_column_over_short_line() {
        let mut v = view("long line\nab\nlong line");
        keys(&mut v, "8l2j");
        assert_eq!(v.cursor, Point::new(2, 8));
    }

    #[test]
    fn normal_mode_motion_drops_a_mouse_selection() {
        let mut v = view("alpha\nbeta\ngamma");
        v.anchor = Some(Point::new(0, 1)); // left behind by a mouse drag
        keys(&mut v, "j");
        assert_eq!(v.anchor, None);
    }

    #[test]
    fn goto_line_drops_a_mouse_selection() {
        let mut v = view("alpha\nbeta\ngamma");
        v.anchor = Some(Point::new(0, 1));
        keys(&mut v, "G");
        assert_eq!(v.anchor, None);
    }

    #[test]
    fn search_repeat_drops_a_mouse_selection() {
        let mut v = view("alpha\nbeta\nalpha");
        v.vim.last_search = Some("alpha".to_string());
        v.vim.last_search_forward = true;
        v.anchor = Some(Point::new(0, 1));
        keys(&mut v, "n");
        assert_eq!(v.anchor, None);
    }

    #[test]
    fn visual_mode_motion_keeps_the_selection() {
        let mut v = view("alpha\nbeta");
        keys(&mut v, "vj");
        assert_eq!(v.anchor, Some(Point::new(0, 0)));
    }

    #[test]
    fn h_resets_desired_column() {
        let mut v = view("long line\nab\nlong line");
        keys(&mut v, "8lj"); // (1, 1), remembering col 8
        keys(&mut v, "h"); // (1, 0)
        keys(&mut v, "j");
        assert_eq!(v.cursor, Point::new(2, 0)); // memory was reset, not col 8
    }

    #[test]
    fn edit_resets_desired_column() {
        let mut v = view("long line\nab\nlong line");
        keys(&mut v, "8lj"); // (1, 1), remembering col 8
        keys(&mut v, "x"); // line becomes "a", cursor (1, 0)
        keys(&mut v, "j");
        assert_eq!(v.cursor, Point::new(2, 0));
    }

    #[test]
    fn dollar_keeps_cursor_at_line_ends() {
        let mut v = view("long\nab\nlonger line");
        keys(&mut v, "$");
        assert_eq!(v.cursor, Point::new(0, 3));
        keys(&mut v, "j");
        assert_eq!(v.cursor, Point::new(1, 1));
        keys(&mut v, "j");
        assert_eq!(v.cursor, Point::new(2, 10));
    }

    #[test]
    fn arrow_keys_remember_column_in_normal_mode() {
        let mut v = view("long line\nab\nlong line");
        keys(&mut v, "8l");
        key(&mut v, Key::Down);
        assert_eq!(v.cursor, Point::new(1, 1));
        key(&mut v, Key::Down);
        assert_eq!(v.cursor, Point::new(2, 8));
    }

    #[test]
    fn arrow_keys_remember_column_in_insert_mode() {
        let mut v = view("long line\nab\nlong line");
        keys(&mut v, "8la"); // insert mode, col 9 (after the last char)
        key(&mut v, Key::Down);
        assert_eq!(v.cursor, Point::new(1, 2));
        key(&mut v, Key::Down);
        assert_eq!(v.cursor, Point::new(2, 9));
    }

    #[test]
    fn l_stops_on_last_char() {
        let mut v = view("ab");
        keys(&mut v, "lllll");
        assert_eq!(v.cursor, Point::new(0, 1)); // never past the last char
    }

    #[test]
    fn zero_and_dollar() {
        let mut v = view("hello");
        keys(&mut v, "$");
        assert_eq!(v.cursor, Point::new(0, 4));
        keys(&mut v, "0");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn caret_goes_to_first_non_blank() {
        let mut v = view("    hello");
        keys(&mut v, "$"); // cursor to end
        assert_eq!(v.cursor, Point::new(0, 8));
        keys(&mut v, "^");
        assert_eq!(v.cursor, Point::new(0, 4)); // first non-blank, not column 0
                                                // `0` still goes to the true line start, distinct from `^`.
        keys(&mut v, "0");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn caret_on_all_blank_line_stays_at_start() {
        let mut v = view("      ");
        keys(&mut v, "$^");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn caret_as_operator_target_deletes_leading_text() {
        // `d^` deletes from the first non-blank up to (exclusive) the cursor.
        let mut v = view("    hello");
        keys(&mut v, "$");
        keys(&mut v, "d^");
        assert_eq!(v.buffer.line(0), "    o");
    }

    #[test]
    fn word_motions() {
        let mut v = view("foo bar baz");
        keys(&mut v, "w");
        assert_eq!(v.cursor, Point::new(0, 4));
        keys(&mut v, "e");
        assert_eq!(v.cursor, Point::new(0, 6)); // end of "bar"
        keys(&mut v, "b");
        assert_eq!(v.cursor, Point::new(0, 4)); // back to start of "bar"
    }

    #[test]
    fn word_motion_crosses_lines() {
        let mut v = view("foo\nbar");
        keys(&mut v, "w");
        assert_eq!(v.cursor, Point::new(1, 0));
    }

    #[test]
    fn gg_and_cap_g() {
        let mut v = view("a\nb\nc\nd");
        keys(&mut v, "G");
        assert_eq!(v.cursor, Point::new(3, 0));
        keys(&mut v, "gg");
        assert_eq!(v.cursor, Point::new(0, 0));
        keys(&mut v, "2G"); // goto line 2
        assert_eq!(v.cursor, Point::new(1, 0));
    }

    // --- gj / gk display-line motions (soft wrap) --------------------------
    //
    // With cols=4, "aaaabbbbcccc" wraps into visual rows [0..4), [4..8),
    // [8..12) — char-col starts 0, 4, 8.

    #[test]
    fn gj_moves_down_one_visual_row_within_a_wrapped_line() {
        let mut v = view("aaaabbbbcccc");
        keys_wrap(&mut v, "gj", 4);
        // Same logical line, cursor advanced to the next visual row's start.
        assert_eq!(v.cursor, Point::new(0, 4));
    }

    #[test]
    fn gk_moves_back_up_one_visual_row() {
        let mut v = view("aaaabbbbcccc");
        keys_wrap(&mut v, "gjgj", 4); // -> row 2, col 8
        assert_eq!(v.cursor, Point::new(0, 8));
        keys_wrap(&mut v, "gk", 4);
        assert_eq!(v.cursor, Point::new(0, 4));
    }

    #[test]
    fn gj_at_last_visual_row_crosses_to_the_next_logical_line() {
        let mut v = view("aaaabbbbcccc\nxyz");
        keys_wrap(&mut v, "gjgjgj", 4); // rows 0->1->2, then next line
        assert_eq!(v.cursor, Point::new(1, 0));
    }

    #[test]
    fn count_gj_moves_several_visual_rows() {
        let mut v = view("aaaabbbbcccc");
        keys_wrap(&mut v, "2gj", 4);
        assert_eq!(v.cursor, Point::new(0, 8));
    }

    #[test]
    fn gj_with_wrap_off_behaves_like_j() {
        let mut v = view("aaaabbbbcccc\nxyz");
        v.wrap = false;
        keys_wrap(&mut v, "gj", 4);
        // No wrapping: a whole logical line down, exactly like `j`.
        assert_eq!(v.cursor, Point::new(1, 0));
    }

    #[test]
    fn gj_preserves_the_display_column_across_rows() {
        let mut v = view("aaaabbbbcccc");
        v.cursor = Point::new(0, 2); // display col 2 of row 0
        keys_wrap(&mut v, "gj", 4);
        assert_eq!(v.cursor, Point::new(0, 6)); // col 2 of row 1
        keys_wrap(&mut v, "gj", 4);
        assert_eq!(v.cursor, Point::new(0, 10)); // col 2 of row 2
    }

    #[test]
    fn gj_keeps_a_sticky_display_column_through_a_short_row() {
        // Rows [0..4), [4..8), [8..10): the last row "cc" is only 2 wide.
        let mut v = view("aaaabbbbcc");
        v.cursor = Point::new(0, 2); // display col 2
        keys_wrap(&mut v, "gjgj", 4);
        // Landed on the short last row, clamped to its final char (col 9).
        assert_eq!(v.cursor, Point::new(0, 9));
        // Coming back up restores the sticky display column (col 2 -> col 6).
        keys_wrap(&mut v, "gk", 4);
        assert_eq!(v.cursor, Point::new(0, 6));
    }

    #[test]
    fn gk_at_buffer_top_and_gj_at_bottom_clamp() {
        let mut v = view("abc\ndef");
        keys_wrap(&mut v, "gk", 4); // already at top: no move, no panic
        assert_eq!(v.cursor, Point::new(0, 0));
        keys_wrap(&mut v, "Ggj", 4); // G -> last line, gj can't go further
        assert_eq!(v.cursor, Point::new(1, 0));
    }

    #[test]
    fn visual_gj_extends_the_selection_by_a_visual_row() {
        let mut v = view("aaaabbbbcccc");
        keys_wrap(&mut v, "vgj", 4);
        assert_eq!(v.vim.mode, Mode::Visual);
        assert_eq!(v.anchor, Some(Point::new(0, 0)));
        assert_eq!(v.cursor, Point::new(0, 4));
        let sel = v.selection().unwrap();
        assert_eq!(sel.ordered(), (Point::new(0, 0), Point::new(0, 4)));
    }

    #[test]
    fn dgj_across_a_logical_line_deletes_both_lines_linewise() {
        let mut v = view("aaaabbbbcccc\nxyz\nlast");
        v.cursor = Point::new(0, 8); // last visual row of line 0
        keys_wrap(&mut v, "dgj", 4); // gj crosses into line 1 -> delete lines 0,1
        assert_eq!(v.buffer.to_string(), "last");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn dgj_within_one_wrapped_line_deletes_charwise_to_the_row_below() {
        // "aaaabbbbcccc" wrapped at 4 → rows aaaa / bbbb / cccc. `dgj` from the
        // start deletes charwise down to the same display column one visual row
        // down (col 0 → col 4), i.e. the first wrapped row.
        let mut v = view("aaaabbbbcccc");
        keys_wrap(&mut v, "dgj", 4);
        assert_eq!(v.buffer.to_string(), "bbbbcccc");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn dgj_within_a_wrapped_line_honors_the_display_column() {
        // From display col 2 on the first row, `dgj` deletes to col 2 of the
        // next row: chars [2, 6) = "aabb", leaving "aa" + "bbcccc".
        let mut v = view("aaaabbbbcccc");
        v.cursor = Point::new(0, 2);
        keys_wrap(&mut v, "dgj", 4);
        assert_eq!(v.buffer.to_string(), "aabbcccc");
    }

    #[test]
    fn ygk_within_a_wrapped_line_yanks_the_display_span() {
        // Upward `ygk` yanks the same exclusive display span and leaves the
        // caret at its start (charwise yank).
        let mut v = view("aaaabbbbcccc");
        v.cursor = Point::new(0, 8); // "cccc" row
        keys_wrap(&mut v, "ygk", 4);
        assert_eq!(v.buffer.to_string(), "aaaabbbbcccc"); // yank doesn't edit
        assert_eq!(v.vim.register, "bbbb"); // exclusive display span [4, 8)
        assert_eq!(v.cursor, Point::new(0, 4));
    }

    #[test]
    fn cgj_within_a_wrapped_line_changes_the_display_span_and_inserts() {
        let mut v = view("aaaabbbbcccc");
        keys_wrap(&mut v, "cgj", 4); // delete [0,4) = "aaaa", enter Insert
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.buffer.to_string(), "bbbbcccc");
        keys_wrap(&mut v, "XY", 4);
        assert_eq!(v.buffer.to_string(), "XYbbbbcccc");
    }

    #[test]
    fn down_arrow_is_wrap_aware_but_j_stays_linewise() {
        // "aaaabbbbcccc" wrapped at 4. Down arrow steps one *visual* row; a bare
        // `j` on the same buffer would leave the single logical line entirely.
        let mut v = view("aaaabbbbcccc\nnext");
        key_wrap(&mut v, Key::Down, 4);
        assert_eq!(v.cursor, Point::new(0, 4)); // one visual row down, same line
        key_wrap(&mut v, Key::Down, 4);
        assert_eq!(v.cursor, Point::new(0, 8)); // still within line 0
                                                // `j` from the top would jump straight to line 1 (logical), proving the
                                                // arrow's display-awareness is distinct.
        let mut v2 = view("aaaabbbbcccc\nnext");
        keys_wrap(&mut v2, "j", 4);
        assert_eq!(v2.cursor.line, 1);
    }

    #[test]
    fn home_and_end_ride_the_visual_row_under_wrap() {
        let mut v = view("aaaabbbbcccc");
        v.cursor = Point::new(0, 6); // middle "bbbb" row (cols 4..8)
        key_wrap(&mut v, Key::Home, 4);
        assert_eq!(v.cursor, Point::new(0, 4)); // start of the visual row
        key_wrap(&mut v, Key::End, 4);
        assert_eq!(v.cursor, Point::new(0, 7)); // last char of the visual row
    }

    #[test]
    fn end_on_the_final_visual_row_lands_on_the_last_character() {
        let mut v = view("aaaabbbbcccc");
        v.cursor = Point::new(0, 9); // "cccc" row (cols 8..12), the final row
        key_wrap(&mut v, Key::End, 4);
        assert_eq!(v.cursor, Point::new(0, 11)); // last real char, clamped
    }

    #[test]
    fn down_arrow_with_operator_pending_stays_linewise() {
        // `d<Down>` keeps `dj` semantics (linewise) even under wrap — only gj/gk
        // are charwise display motions.
        let mut v = view("aaaabbbbcccc\nnext\ntail");
        key_wrap(&mut v, Key::Char('d'), 4);
        key_wrap(&mut v, Key::Down, 4);
        assert_eq!(v.buffer.to_string(), "tail"); // deleted lines 0 and 1
    }

    #[test]
    fn insert_mode_down_arrow_follows_visual_rows() {
        let mut v = view("aaaabbbbcccc");
        keys_wrap(&mut v, "i", 4); // enter Insert at col 0
        key_wrap(&mut v, Key::Down, 4);
        assert_eq!(v.cursor, Point::new(0, 4)); // one visual row down
    }

    #[test]
    fn plain_j_and_k_still_work_with_the_wrap_width_param() {
        let mut v = view("abc\ndef\nghi");
        keys(&mut v, "jj");
        assert_eq!(v.cursor.line, 2);
        keys(&mut v, "k");
        assert_eq!(v.cursor.line, 1);
    }

    /// An `n`-line buffer, one number per line.
    fn numbered(n: usize) -> String {
        (0..n).map(|i| i.to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn z_scroll_aligns_viewport_to_cursor() {
        let text = numbered(30);
        let vis = 10; // 10 visible rows; max_scroll_top = 30 - 10 = 20

        // `zt` puts the cursor's line at the top of the viewport.
        let mut v = view(&text);
        keys_vis(&mut v, "15Gzt", vis); // cursor -> line 14
        assert_eq!(v.cursor.line, 14);
        assert_eq!(v.scroll.top, 14);

        // `zz` centers the cursor's line.
        let mut v = view(&text);
        keys_vis(&mut v, "15Gzz", vis);
        assert_eq!(v.scroll.top, 14 - vis / 2); // 9

        // `zb` puts the cursor's line at the bottom of the viewport.
        let mut v = view(&text);
        keys_vis(&mut v, "15Gzb", vis);
        assert_eq!(v.scroll.top, 14 + 1 - vis); // 5

        // The cursor never moves.
        assert_eq!(v.cursor.line, 14);
    }

    #[test]
    fn z_scroll_clamps_to_buffer_bounds() {
        let text = numbered(30);
        let vis = 10;

        // `zt` near the end clamps so the view never scrolls past the last line.
        let mut v = view(&text);
        keys_vis(&mut v, "Gzt", vis); // cursor -> line 29
        assert_eq!(v.scroll.top, 20); // clamped to max_scroll_top

        // `zz`/`zb` at the top of the buffer saturate at 0.
        let mut v = view(&text);
        keys_vis(&mut v, "ggzz", vis);
        assert_eq!(v.scroll.top, 0);
        let mut v = view(&text);
        keys_vis(&mut v, "ggzb", vis);
        assert_eq!(v.scroll.top, 0);
    }

    /// Feed one Ctrl chord with an explicit viewport height.
    fn ctrl_vis(v: &mut EditorView, c: char, visible_lines: usize) {
        handle(
            v,
            Key::Ctrl(c),
            visible_lines,
            WIDE,
            &mut InMemoryClipboard::default(),
        );
    }

    #[test]
    fn ctrl_d_and_ctrl_u_move_cursor_half_a_viewport() {
        let text = numbered(30);
        let vis = 10; // half a viewport = 5 lines

        let mut v = view(&text);
        ctrl_vis(&mut v, 'd', vis); // line 0 -> 5
        assert_eq!(v.cursor.line, 5);
        ctrl_vis(&mut v, 'd', vis); // 5 -> 10
        assert_eq!(v.cursor.line, 10);
        ctrl_vis(&mut v, 'u', vis); // 10 -> 5
        assert_eq!(v.cursor.line, 5);
    }

    #[test]
    fn ctrl_d_then_ctrl_u_is_a_round_trip() {
        // Parity: from a mid-buffer line, down-half then up-half lands back.
        let text = numbered(40);
        let vis = 12;
        let mut v = view(&text);
        keys_vis(&mut v, "20G", vis); // cursor -> line 19
        ctrl_vis(&mut v, 'd', vis);
        ctrl_vis(&mut v, 'u', vis);
        assert_eq!(v.cursor.line, 19);
    }

    #[test]
    fn ctrl_d_and_ctrl_u_clamp_at_buffer_ends() {
        let text = numbered(8);
        let vis = 10; // half = 5, larger than the remaining lines
        let mut v = view(&text);
        ctrl_vis(&mut v, 'd', vis);
        assert_eq!(v.cursor.line, 5);
        ctrl_vis(&mut v, 'd', vis); // would be 10, clamps to last line 7
        assert_eq!(v.cursor.line, 7);
        ctrl_vis(&mut v, 'u', vis); // 7 -> 2
        assert_eq!(v.cursor.line, 2);
        ctrl_vis(&mut v, 'u', vis); // would be -3, clamps to 0
        assert_eq!(v.cursor.line, 0);
    }

    #[test]
    fn ctrl_d_extends_a_visual_selection() {
        let text = numbered(30);
        let vis = 10;
        let mut v = view(&text);
        keys_vis(&mut v, "v", vis); // enter Visual at line 0
        ctrl_vis(&mut v, 'd', vis); // extend down half a page
        assert_eq!(v.cursor.line, 5);
        let sel = v.selection().expect("visual selection is active");
        let (start, end) = sel.ordered();
        assert_eq!(start.line, 0);
        assert_eq!(end.line, 5);
    }

    #[test]
    fn x_deletes_char_under_cursor() {
        let mut v = view("hello");
        keys(&mut v, "x");
        assert_eq!(v.buffer.to_string(), "ello");
    }

    #[test]
    fn count_x_deletes_several() {
        let mut v = view("hello");
        keys(&mut v, "3x");
        assert_eq!(v.buffer.to_string(), "lo");
    }

    #[test]
    fn s_substitutes_char_and_enters_insert() {
        let mut v = view("hello");
        keys(&mut v, "s"); // delete 'h', enter Insert
        assert_eq!(v.vim.mode, Mode::Insert);
        keys(&mut v, "X"); // typed in Insert mode
        assert_eq!(v.buffer.to_string(), "Xello");
    }

    #[test]
    fn count_s_substitutes_several_chars() {
        let mut v = view("hello");
        keys(&mut v, "3sY"); // delete 'hel', enter Insert, type 'Y'
        assert_eq!(v.buffer.to_string(), "Ylo");
        assert_eq!(v.vim.mode, Mode::Insert);
    }

    #[test]
    fn s_at_line_end_inserts_after_last_remaining_char() {
        let mut v = view("ab");
        keys(&mut v, "ls"); // on 'b', substitute it
        keys(&mut v, "Z");
        assert_eq!(v.buffer.to_string(), "aZ");
    }

    #[test]
    fn s_on_empty_line_just_enters_insert() {
        let mut v = view("\ntwo");
        keys(&mut v, "s"); // nothing to delete on the empty first line
        assert_eq!(v.vim.mode, Mode::Insert);
        keys(&mut v, "x");
        assert_eq!(v.buffer.to_string(), "x\ntwo");
    }

    #[test]
    fn s_in_visual_mode_changes_the_selection() {
        let mut v = view("hello");
        keys(&mut v, "vls"); // select "he", substitute it
        assert_eq!(v.vim.mode, Mode::Insert);
        keys(&mut v, "X");
        assert_eq!(v.buffer.to_string(), "Xllo");
    }

    #[test]
    fn dd_deletes_line() {
        let mut v = view("one\ntwo\nthree");
        keys(&mut v, "j"); // on "two"
        keys(&mut v, "dd");
        assert_eq!(v.buffer.to_string(), "one\nthree");
        assert_eq!(v.cursor.line, 1);
    }

    #[test]
    fn count_dd_deletes_several_lines() {
        let mut v = view("a\nb\nc\nd");
        keys(&mut v, "2dd");
        assert_eq!(v.buffer.to_string(), "c\nd");
    }

    #[test]
    fn dd_on_last_line_leaves_no_blank() {
        let mut v = view("one\ntwo");
        keys(&mut v, "G"); // last line
        keys(&mut v, "dd");
        assert_eq!(v.buffer.to_string(), "one");
    }

    #[test]
    fn dw_deletes_word() {
        let mut v = view("foo bar");
        keys(&mut v, "dw");
        assert_eq!(v.buffer.to_string(), "bar");
    }

    #[test]
    fn dw_on_last_word_does_not_join_next_line() {
        let mut v = view("foo\nbar");
        keys(&mut v, "dw"); // "foo" is the whole first line
        assert_eq!(v.buffer.to_string(), "\nbar"); // line emptied, not joined
    }

    #[test]
    fn cap_d_deletes_to_end_of_line() {
        let mut v = view("hello world");
        keys(&mut v, "lllll"); // col 5 (the space)
        keys(&mut v, "D");
        assert_eq!(v.buffer.to_string(), "hello");
    }

    #[test]
    fn cc_clears_line_and_enters_insert() {
        let mut v = view("one\ntwo");
        keys(&mut v, "cc");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.buffer.to_string(), "\ntwo");
        keys(&mut v, "X");
        assert_eq!(v.buffer.to_string(), "X\ntwo");
    }

    #[test]
    fn cap_s_clears_line_and_enters_insert() {
        let mut v = view("one\ntwo");
        keys(&mut v, "ll"); // cursor mid-line — S still clears the whole line
        keys(&mut v, "S");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.buffer.to_string(), "\ntwo");
        keys(&mut v, "X");
        assert_eq!(v.buffer.to_string(), "X\ntwo");
    }

    #[test]
    fn count_cap_s_clears_several_lines() {
        let mut v = view("one\ntwo\nthree");
        keys(&mut v, "2S"); // clear two lines into one empty line
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.buffer.to_string(), "\nthree");
        keys(&mut v, "Y");
        assert_eq!(v.buffer.to_string(), "Y\nthree");
    }

    #[test]
    fn cap_c_changes_to_end_of_line() {
        let mut v = view("hello world");
        keys(&mut v, "lllll"); // at the space
        keys(&mut v, "C");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.buffer.to_string(), "hello");
    }

    #[test]
    fn cap_j_joins_next_line_with_a_space() {
        let mut v = view("one\ntwo");
        keys(&mut v, "J");
        assert_eq!(v.buffer.to_string(), "one two");
        // Caret lands on the inserted space (col == old first-line length).
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn cap_j_strips_leading_whitespace_of_the_joined_line() {
        let mut v = view("one\n    two");
        keys(&mut v, "J");
        assert_eq!(v.buffer.to_string(), "one two");
    }

    #[test]
    fn cap_j_adds_no_space_before_a_close_paren() {
        let mut v = view("foo(\n)");
        keys(&mut v, "J");
        assert_eq!(v.buffer.to_string(), "foo()");
    }

    #[test]
    fn cap_j_does_not_double_a_trailing_space() {
        let mut v = view("one \ntwo");
        keys(&mut v, "J");
        assert_eq!(v.buffer.to_string(), "one two");
    }

    #[test]
    fn count_cap_j_joins_several_lines() {
        let mut v = view("a\nb\nc\nd");
        keys(&mut v, "3J"); // join three lines: a, b, c
        assert_eq!(v.buffer.to_string(), "a b c\nd");
        assert_eq!(v.cursor, Point::new(0, 3)); // on the last seam
    }

    #[test]
    fn cap_j_on_last_line_is_a_no_op() {
        let mut v = view("one\ntwo");
        keys(&mut v, "G"); // last line
        keys(&mut v, "J");
        assert_eq!(v.buffer.to_string(), "one\ntwo");
    }

    #[test]
    fn cap_j_undoes_in_one_step() {
        let mut v = view("a\nb\nc");
        keys(&mut v, "3J");
        assert_eq!(v.buffer.to_string(), "a b c");
        v.undo();
        assert_eq!(v.buffer.to_string(), "a\nb\nc");
    }

    #[test]
    fn visual_cap_j_joins_the_selected_lines() {
        let mut v = view("a\nb\nc\nd");
        keys(&mut v, "Vj"); // select lines 0..1
        keys(&mut v, "J");
        assert_eq!(v.buffer.to_string(), "a b\nc\nd");
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn count_motion_moves_multiple() {
        let mut v = view("abcdef");
        keys(&mut v, "3l");
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn escape_cancels_pending_operator() {
        let mut v = view("hello");
        keys(&mut v, "d");
        esc(&mut v);
        keys(&mut v, "w"); // plain motion, not a delete
        assert_eq!(v.buffer.to_string(), "hello");
    }

    #[test]
    fn yy_and_p_duplicate_a_line() {
        let mut v = view("one\ntwo");
        keys(&mut v, "yy"); // yank "one"
        keys(&mut v, "p"); // paste below
        assert_eq!(v.buffer.to_string(), "one\none\ntwo");
        assert_eq!(v.cursor.line, 1);
    }

    #[test]
    fn cap_p_pastes_line_above() {
        let mut v = view("one\ntwo");
        keys(&mut v, "j"); // on "two"
        keys(&mut v, "yy");
        keys(&mut v, "P"); // paste above "two"
        assert_eq!(v.buffer.to_string(), "one\ntwo\ntwo");
    }

    #[test]
    fn dd_then_p_moves_a_line() {
        let mut v = view("one\ntwo\nthree");
        keys(&mut v, "dd"); // delete "one" into the register
        keys(&mut v, "p"); // paste it below "two"
        assert_eq!(v.buffer.to_string(), "two\none\nthree");
    }

    #[test]
    fn yw_then_p_pastes_charwise() {
        let mut v = view("foo bar");
        keys(&mut v, "yw"); // yank "foo "
        keys(&mut v, "$"); // end of line
        keys(&mut v, "p"); // paste after 'r'
        assert_eq!(v.buffer.to_string(), "foo barfoo ");
    }

    #[test]
    fn x_yanks_so_p_pastes_it() {
        let mut v = view("abc");
        keys(&mut v, "x"); // delete 'a' (register = "a")
        keys(&mut v, "p"); // paste after 'b'
        assert_eq!(v.buffer.to_string(), "bac");
    }

    #[test]
    fn r_replaces_char() {
        let mut v = view("cat");
        keys(&mut v, "r");
        keys(&mut v, "b"); // replace 'c' with 'b'
        assert_eq!(v.buffer.to_string(), "bat");
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn count_r_replaces_several() {
        let mut v = view("aaaa");
        keys(&mut v, "3r");
        keys(&mut v, "x");
        assert_eq!(v.buffer.to_string(), "xxxa");
    }

    #[test]
    fn visual_select_and_delete() {
        let mut v = view("hello");
        keys(&mut v, "v"); // visual, selecting 'h'
        keys(&mut v, "ll"); // extend over 'e','l'
        keys(&mut v, "d"); // delete selection (inclusive: h,e,l)
        assert_eq!(v.buffer.to_string(), "lo");
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn visual_yank_then_paste() {
        let mut v = view("hello");
        keys(&mut v, "vll"); // select "hel"
        keys(&mut v, "y"); // yank, back to normal at start
        assert_eq!(v.vim.mode, Mode::Normal);
        keys(&mut v, "$p"); // paste after last char
        assert_eq!(v.buffer.to_string(), "hellohel");
    }

    // --- clipboard bridge -------------------------------------------------

    #[test]
    fn yy_writes_through_to_the_clipboard() {
        let mut v = view("one line\ntwo");
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "yy");
        assert_eq!(clip.get().as_deref(), Some("one line"));
    }

    #[test]
    fn visual_yank_writes_through_to_the_clipboard() {
        let mut v = view("hello");
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "vlly");
        assert_eq!(clip.get().as_deref(), Some("hel"));
    }

    #[test]
    fn dd_writes_through_to_the_clipboard() {
        let mut v = view("one\ntwo");
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "dd");
        assert_eq!(clip.get().as_deref(), Some("one"));
    }

    #[test]
    fn x_writes_through_to_the_clipboard() {
        let mut v = view("abc");
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "x");
        assert_eq!(clip.get().as_deref(), Some("a"));
    }

    #[test]
    fn p_pastes_external_clipboard_text_charwise() {
        let mut v = view("one\ntwo");
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "yy"); // register = "one", linewise
        clip.set("EXT"); // the clipboard changed under us (e.g. another app)
        keys_clip(&mut v, &mut clip, "p");
        // External text differs from the register: characterwise paste.
        assert_eq!(v.buffer.to_string(), "oEXTne\ntwo");
        assert_eq!(v.cursor, Point::new(0, 3)); // on the last pasted char
    }

    #[test]
    fn p_stays_linewise_when_clipboard_matches_register() {
        let mut v = view("one\ntwo");
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "yyp");
        assert_eq!(v.buffer.to_string(), "one\none\ntwo");
    }

    #[test]
    fn p_falls_back_to_register_when_clipboard_is_empty() {
        let mut v = view("one\ntwo");
        let mut yank_clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut yank_clip, "yy");
        // Paste through a clipboard that has nothing (e.g. the OS pasteboard
        // was unavailable): the internal register still works, linewise.
        let mut empty_clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut empty_clip, "p");
        assert_eq!(v.buffer.to_string(), "one\none\ntwo");
    }

    // --- search: / ? n N * --------------------------------------------------

    #[test]
    fn slash_and_question_open_the_search_prompt() {
        let mut v = view("hello");
        let mut clip = InMemoryClipboard::default();
        assert_eq!(
            handle(&mut v, Key::Char('/'), 50, WIDE, &mut clip),
            Action::OpenSearch { forward: true }
        );
        assert_eq!(
            handle(&mut v, Key::Char('?'), 50, WIDE, &mut clip),
            Action::OpenSearch { forward: false }
        );
    }

    #[test]
    fn star_searches_forward_for_word_under_cursor() {
        let mut v = view("foo bar foo baz");
        keys(&mut v, "*");
        assert_eq!(v.cursor, Point::new(0, 8));
        assert_eq!(v.vim.last_search.as_deref(), Some("foo"));
        assert!(v.vim.last_search_forward);
        assert!(v.vim.search_hl);
    }

    #[test]
    fn star_on_non_word_char_is_a_noop() {
        let mut v = view(". foo . foo");
        keys(&mut v, "*");
        assert_eq!(v.cursor, Point::new(0, 0));
        assert_eq!(v.vim.last_search, None);
    }

    #[test]
    fn star_matches_whole_words_only() {
        // The embedded "foo" in "foobar" is skipped; the whole word wraps back.
        let mut v = view("foo foobar\nfoo");
        keys(&mut v, "*");
        assert_eq!(v.cursor, Point::new(1, 0));
        assert!(v.vim.last_search_word);
        // `n` keeps the whole-word semantics: wraps to (0,0), not into foobar.
        keys(&mut v, "n");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn hash_searches_backward_for_word_under_cursor() {
        let mut v = view("foo bar foo baz");
        v.cursor = Point::new(0, 8); // on the second "foo"
        keys(&mut v, "#");
        assert_eq!(v.cursor, Point::new(0, 0));
        assert_eq!(v.vim.last_search.as_deref(), Some("foo"));
        assert!(!v.vim.last_search_forward);
        assert!(v.vim.last_search_word);
        // `n` continues backward (wraps to the later match).
        keys(&mut v, "n");
        assert_eq!(v.cursor, Point::new(0, 8));
    }

    #[test]
    fn hash_on_non_word_char_is_a_noop() {
        let mut v = view(". foo . foo");
        keys(&mut v, "#");
        assert_eq!(v.cursor, Point::new(0, 0));
        assert_eq!(v.vim.last_search, None);
    }

    #[test]
    fn n_repeats_search_and_wraps() {
        let mut v = view("foo bar foo");
        keys(&mut v, "*"); // -> (0, 8)
        keys(&mut v, "n"); // wraps -> (0, 0)
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn cap_n_repeats_search_reversed() {
        let mut v = view("foo bar foo");
        keys(&mut v, "*"); // -> (0, 8)
        keys(&mut v, "N"); // backward -> (0, 0)
        assert_eq!(v.cursor, Point::new(0, 0));
        keys(&mut v, "N"); // backward again, wraps -> (0, 8)
        assert_eq!(v.cursor, Point::new(0, 8));
    }

    #[test]
    fn n_follows_a_backward_search_direction() {
        let mut v = view("foo\nfoo\nfoo");
        v.vim.last_search = Some("foo".to_string());
        v.vim.last_search_forward = false;
        keys(&mut v, "n"); // backward from (0,0), wraps to the bottom
        assert_eq!(v.cursor, Point::new(2, 0));
        keys(&mut v, "N"); // reversed = forward -> (0, 0)
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn count_n_repeats_several_times() {
        let mut v = view("foo foo foo foo");
        keys(&mut v, "*"); // -> (0, 4)
        keys(&mut v, "2n"); // -> (0, 12)
        assert_eq!(v.cursor, Point::new(0, 12));
    }

    #[test]
    fn n_without_a_search_is_a_noop() {
        let mut v = view("foo bar");
        keys(&mut v, "n");
        assert_eq!(v.cursor, Point::new(0, 0));
        assert!(!v.vim.search_hl);
    }

    #[test]
    fn n_rearms_highlight_after_noh() {
        let mut v = view("foo bar foo");
        keys(&mut v, "*");
        v.vim.search_hl = false; // what `:noh` does
        keys(&mut v, "n");
        assert!(v.vim.search_hl); // pattern was remembered and re-highlighted
    }

    // --- undo / redo: u and Ctrl+R ------------------------------------------

    #[test]
    fn u_undoes_the_last_edit() {
        let mut v = view("hello");
        keys(&mut v, "x");
        assert_eq!(v.buffer.to_string(), "ello");
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "hello");
    }

    #[test]
    fn u_with_nothing_to_undo_is_a_noop() {
        let mut v = view("hi");
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "hi");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn count_u_undoes_count_times() {
        let mut v = view("abcdef");
        keys(&mut v, "xxx"); // three separate transactions
        assert_eq!(v.buffer.to_string(), "def");
        keys(&mut v, "3u");
        assert_eq!(v.buffer.to_string(), "abcdef");
    }

    #[test]
    fn u_undoes_a_whole_insert_run_in_one_step() {
        let mut v = view("xy");
        keys(&mut v, "iabc");
        esc(&mut v);
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "xy");
    }

    #[test]
    fn escape_ends_the_undo_run_even_when_appending_in_place() {
        // `a` after Esc re-enters insert at the exact char index where the
        // previous run stopped; without an explicit break the 'c' would
        // coalesce in and `u` would eat both runs.
        let mut v = view("");
        keys(&mut v, "iab");
        esc(&mut v); // "ab", caret on 'b'
        keys(&mut v, "ac");
        esc(&mut v); // "abc"
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "ab");
    }

    #[test]
    fn backspace_in_insert_folds_into_the_session_undo() {
        // vim-parity: a backspace mid-insert is part of the same insert session,
        // so one `u` undoes the whole session (typed text and all), not just
        // back to the backspace. (FINDINGS bug 8.)
        let mut v = view("Z");
        keys(&mut v, "iab"); // "abZ"
        key(&mut v, Key::Backspace); // "aZ"
        esc(&mut v);
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "Z");
    }

    #[test]
    fn newline_in_insert_folds_into_the_session_undo() {
        // An <Enter> pressed during insert is part of the one session, so a
        // single `u` removes everything typed, the split included.
        let mut v = view("");
        keys(&mut v, "ia");
        key(&mut v, Key::Enter);
        keys(&mut v, "b"); // "a\nb"
        esc(&mut v);
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "");
    }

    #[test]
    fn change_word_and_typing_undo_in_one_step() {
        // vim-parity: `cw` deletes the word and the typed replacement is part of
        // the same change command, so one `u` restores the original word (the
        // delete and the insert session collapse into a single undo step).
        let mut v = view("foo\nbar");
        keys(&mut v, "cwbaz"); // "baz\nbar"
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "baz\nbar");
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "foo\nbar");
    }

    #[test]
    fn cw_keeps_the_trailing_whitespace() {
        // vim-parity: `cw` on a non-blank behaves like `ce` — it changes to the
        // end of the word and leaves the following space (a word does not
        // include the whitespace after it). (FINDINGS remaining #1.)
        let mut v = view("foo bar");
        keys(&mut v, "cwbaz");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "baz bar");
    }

    #[test]
    fn cw_on_the_last_char_of_a_word_changes_only_that_char() {
        // Unlike `ce`, `cw` with the cursor on a word's last char changes just
        // that char (vim's `end_word` `stop` flag): it does not leap to the next
        // word's end the way `e` does.
        let mut v = view("ab cd");
        keys(&mut v, "l"); // cursor on 'b', the last char of "ab"
        keys(&mut v, "cwX");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "aX cd");
    }

    #[test]
    fn cw_count_spans_words_keeping_the_trailing_space() {
        // `c2w` on a non-blank changes through the end of the second word,
        // still stopping before the space that follows it.
        let mut v = view("foo bar baz");
        keys(&mut v, "c2wX");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "X baz");
    }

    #[test]
    fn cw_on_an_empty_line_enters_insert_without_joining() {
        // vim-parity: `cw` never crosses the newline. On an empty line it changes
        // nothing but still opens an empty insert (it does not pull up the next
        // line the way `dw` would).
        let mut v = view("\nqux");
        keys(&mut v, "cwZ");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "Z\nqux");
    }

    #[test]
    fn cw_on_a_blank_line_changes_the_blanks_only() {
        // On a blanks-only line `cw` changes the blanks up to end-of-line and
        // stays put (it does not join the following line).
        let mut v = view("  \nx");
        keys(&mut v, "cwZ");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "Z\nx");
    }

    #[test]
    fn cw_on_whitespace_changes_like_dw() {
        // With the cursor on whitespace `cw` has no special case: it changes the
        // blanks up to the next word (the normal `w` motion), like `dw`.
        let mut v = view("foo   bar");
        keys(&mut v, "el"); // 'e' -> 'o' (end of foo, col 2); 'l' -> first space col 3
        assert_eq!(v.cursor, Point::new(0, 3));
        keys(&mut v, "cwX");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "fooXbar");
    }

    #[test]
    fn cw_on_punctuation_changes_the_punctuation_run() {
        // Word classes are word / punctuation / blank; `cw` on a punctuation run
        // changes to the end of that run, keeping the following space.
        let mut v = view("a += b");
        keys(&mut v, "ll"); // cursor on '+' (col 2)
        keys(&mut v, "cwX");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "a X b");
    }

    #[test]
    fn c_with_a_clamping_zero_width_motion_enters_insert() {
        // vim-parity: `c0`/`ch`/`c5h` at column 0 change nothing but still open an
        // empty insert (vim's `0`/`h` stop at the boundary rather than failing).
        for motion in ["0", "h", "5h"] {
            let mut v = view("abc def");
            keys(&mut v, &format!("c{motion}X"));
            esc(&mut v);
            assert_eq!(
                v.buffer.to_string(),
                "Xabc def",
                "c{motion} should insert at col 0"
            );
        }
    }

    #[test]
    fn cb_at_buffer_start_cancels_without_inserting() {
        // Word-backward motions *fail* when they can't move, so `cb`/`cB` at the
        // start of the buffer cancel the operator (no empty insert), unlike `ch`.
        let mut v = view("abc def");
        keys(&mut v, "cbX");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "abc def"); // 'X' never inserted
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn c_followed_by_a_non_motion_cancels_the_operator() {
        // vim-parity: an operator pending on `c` (or `d`/`y`) is cancelled and
        // the key swallowed when the next key isn't a motion/`g`-chord/double.
        // `cC`, `cx`, `cr`, `ci`, `dc`, … are all no-ops, not "run the 2nd key".
        for prog in ["cC", "cx", "cr", "ci", "cv", "dc", "c5000C"] {
            let mut v = view("abc def");
            keys(&mut v, prog);
            keys(&mut v, "Z"); // any follow-up typing must not land as text
            esc(&mut v);
            assert_eq!(v.buffer.to_string(), "abc def", "{prog} should be a no-op");
            assert_eq!(v.vim.mode, Mode::Normal, "{prog} should stay in Normal");
        }
    }

    #[test]
    fn c_count_dollar_past_the_last_line_is_a_no_op() {
        // `c2$`/`d2$` needs to move a line down; on the last line that fails and
        // the whole operator is cancelled (like `c2j`), rather than changing the
        // current line to its end.
        let mut v = view("abcdef");
        keys(&mut v, "c2$X");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "abcdef");
        assert_eq!(v.vim.mode, Mode::Normal);

        // But when it *can* move down, a counted `$` clamps to the last line.
        let mut v = view("ab\ncd");
        keys(&mut v, "c3$X");
        esc(&mut v);
        assert_eq!(v.buffer.to_string(), "X");
    }

    #[test]
    fn ctrl_r_lands_on_the_start_of_the_redone_insert() {
        // vim-parity: `<C-R>` places the caret on the first re-inserted
        // character (start of the change), not past the last one like a plain
        // editor redo would.
        let mut v = view("abc");
        keys(&mut v, "AYZ"); // append -> "abcYZ", caret past 'Z'
        esc(&mut v);
        keys(&mut v, "u"); // -> "abc"
        key(&mut v, Key::Ctrl('r')); // redo -> "abcYZ"
        assert_eq!(v.buffer.to_string(), "abcYZ");
        assert_eq!(v.cursor, Point::new(0, 3)); // on 'Y', the first re-inserted char
    }

    #[test]
    fn u_clamps_the_cursor_for_normal_mode() {
        let mut v = view("abc");
        keys(&mut v, "A!"); // append '!' at line end
        esc(&mut v); // "abc!", caret on '!'
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "abc");
        // The buffer restores an insert-style position (0, 3); Normal mode
        // pulls it back onto the last character.
        assert_eq!(v.cursor, Point::new(0, 2));
    }

    #[test]
    fn ctrl_r_redoes() {
        let mut v = view("hello");
        keys(&mut v, "xu");
        assert_eq!(v.buffer.to_string(), "hello");
        key(&mut v, Key::Ctrl('r'));
        assert_eq!(v.buffer.to_string(), "ello");
    }

    #[test]
    fn count_ctrl_r_redoes_count_times() {
        let mut v = view("abcdef");
        keys(&mut v, "xxx3u"); // back to "abcdef"
        keys(&mut v, "2");
        key(&mut v, Key::Ctrl('r'));
        assert_eq!(v.buffer.to_string(), "cdef");
    }

    #[test]
    fn u_in_insert_mode_types_a_literal_u() {
        let mut v = view("x");
        keys(&mut v, "iu");
        assert_eq!(v.buffer.to_string(), "ux");
        assert_eq!(v.vim.mode, Mode::Insert);
    }

    #[test]
    fn u_in_visual_mode_lowercases_the_selection() {
        let mut v = view("HELLO");
        keys(&mut v, "vll"); // select H, E, L
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "helLO");
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn capital_u_in_visual_mode_uppercases_the_selection() {
        let mut v = view("hello");
        keys(&mut v, "vll"); // select h, e, l
        keys(&mut v, "U");
        assert_eq!(v.buffer.to_string(), "HELlo");
    }

    #[test]
    fn tilde_in_visual_mode_toggles_case() {
        let mut v = view("Hello");
        keys(&mut v, "v$"); // whole line (inclusive)
        keys(&mut v, "~");
        assert_eq!(v.buffer.to_string(), "hELLO");
    }

    #[test]
    fn tilde_in_normal_mode_toggles_char_and_advances() {
        let mut v = view("hello");
        keys(&mut v, "~");
        assert_eq!(v.buffer.to_string(), "Hello");
        assert_eq!(v.cursor, Point::new(0, 1)); // advanced past the toggled char
        keys(&mut v, "~");
        assert_eq!(v.buffer.to_string(), "HEllo");
        assert_eq!(v.cursor, Point::new(0, 2));
    }

    #[test]
    fn counted_tilde_toggles_multiple_chars() {
        let mut v = view("aBcDe");
        keys(&mut v, "3~");
        assert_eq!(v.buffer.to_string(), "AbCDe");
        assert_eq!(v.cursor, Point::new(0, 3)); // advanced past all three toggled chars
    }

    #[test]
    fn tilde_leaves_non_letters_and_clamps_at_line_end() {
        // Count past the end toggles only what's there; digits/symbols unchanged.
        let mut v = view("a1!z");
        keys(&mut v, "9~");
        assert_eq!(v.buffer.to_string(), "A1!Z");
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn tilde_on_empty_line_is_a_no_op() {
        let mut v = view("");
        keys(&mut v, "~");
        assert_eq!(v.buffer.to_string(), "");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn capital_v_enters_visual_line_and_deletes_whole_lines() {
        let mut v = view("one\ntwo\nthree");
        v.cursor = Point::new(1, 1); // somewhere on "two"
        keys(&mut v, "Vd");
        assert_eq!(v.buffer.to_string(), "one\nthree");
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn visual_line_extends_over_motions_then_deletes() {
        let mut v = view("a\nb\nc\nd");
        keys(&mut v, "Vjjd"); // select lines 0..=2
        assert_eq!(v.buffer.to_string(), "d");
    }

    #[test]
    fn visual_line_yank_pastes_as_a_new_line() {
        let mut v = view("one\ntwo");
        keys(&mut v, "Vy"); // yank line 0 linewise
        keys(&mut v, "p"); // paste below
        assert_eq!(v.buffer.to_string(), "one\none\ntwo");
    }

    #[test]
    fn o_swaps_the_visual_ends() {
        let mut v = view("hello world");
        keys(&mut v, "v"); // anchor at col 0
        keys(&mut v, "ll"); // head at col 2
        keys(&mut v, "o"); // swap: head now at col 0, anchor at col 2
        assert_eq!(v.cursor, Point::new(0, 0));
        assert_eq!(v.anchor, Some(Point::new(0, 2)));
        keys(&mut v, "h"); // extending now grows the left end
        assert_eq!(v.cursor, Point::new(0, 0)); // already at start, stays
    }

    #[test]
    fn visual_indent_adds_one_level_to_selected_lines() {
        let mut v = view("a\nb\nc");
        keys(&mut v, "Vj>"); // indent lines 0 and 1
        assert_eq!(v.buffer.to_string(), "    a\n    b\nc");
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn visual_dedent_removes_one_level() {
        let mut v = view("        a\n    b\nc");
        keys(&mut v, "Vj<"); // dedent lines 0 and 1 by 4 spaces
        assert_eq!(v.buffer.to_string(), "    a\nb\nc");
    }

    #[test]
    fn visual_indent_is_one_undo_step() {
        let mut v = view("a\nb");
        keys(&mut v, "Vj>");
        keys(&mut v, "u");
        assert_eq!(v.buffer.to_string(), "a\nb");
    }

    #[test]
    fn v_in_visual_line_switches_to_charwise() {
        let mut v = view("hello");
        keys(&mut v, "V");
        assert_eq!(v.vim.mode, Mode::VisualLine);
        keys(&mut v, "v");
        assert_eq!(v.vim.mode, Mode::Visual);
    }

    #[test]
    fn ctrl_r_in_insert_mode_inserts_nothing() {
        let mut v = view("x");
        keys(&mut v, "i");
        key(&mut v, Key::Ctrl('r'));
        assert_eq!(v.buffer.to_string(), "x");
        assert_eq!(v.vim.mode, Mode::Insert);
    }

    #[test]
    fn visual_toggles_off() {
        let mut v = view("hello");
        keys(&mut v, "v");
        assert_eq!(v.vim.mode, Mode::Visual);
        keys(&mut v, "v");
        assert_eq!(v.vim.mode, Mode::Normal);
        assert!(v.selection().is_none());
    }

    // --- % bracket matching ----------------------------------------------

    #[test]
    fn percent_jumps_from_open_to_close() {
        let mut v = view("(a + b)");
        keys(&mut v, "%"); // cursor on '(' at col 0
        assert_eq!(v.cursor, Point::new(0, 6)); // the ')'
    }

    #[test]
    fn percent_jumps_from_close_back_to_open() {
        let mut v = view("(a + b)");
        v.cursor = Point::new(0, 6); // on ')'
        keys(&mut v, "%");
        assert_eq!(v.cursor, Point::new(0, 0)); // the '('
    }

    #[test]
    fn percent_seeks_the_first_bracket_after_the_cursor() {
        let mut v = view("foo(bar)"); // cursor at col 0, no bracket there
        keys(&mut v, "%"); // seeks forward on the line to '(' then matches it
        assert_eq!(v.cursor, Point::new(0, 7)); // the ')'
    }

    #[test]
    fn percent_matches_across_lines_with_nesting() {
        let mut v = view("fn f() {\n    g(x)\n}");
        v.cursor = Point::new(0, 7); // on the '{'
        keys(&mut v, "%");
        assert_eq!(v.cursor, Point::new(2, 0)); // the closing '}'
    }

    #[test]
    fn percent_respects_nested_pairs() {
        let mut v = view("(a (b) c)");
        keys(&mut v, "%"); // outer '(' at 0
        assert_eq!(v.cursor, Point::new(0, 8)); // outer ')', not the inner one
    }

    #[test]
    fn percent_with_no_bracket_on_line_is_a_noop() {
        let mut v = view("plain text");
        keys(&mut v, "%");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn percent_with_unbalanced_bracket_is_a_noop() {
        let mut v = view("(a + b");
        keys(&mut v, "%");
        assert_eq!(v.cursor, Point::new(0, 0)); // no match: cursor stays
    }

    #[test]
    fn d_percent_deletes_through_the_matching_bracket_inclusive() {
        let mut v = view("(abc)def");
        keys(&mut v, "d%"); // on '(': delete (abc) inclusive
        assert_eq!(v.buffer.to_string(), "def");
    }

    #[test]
    fn d_percent_from_closing_bracket_deletes_the_pair() {
        let mut v = view("x(abc)y");
        v.cursor = Point::new(0, 5); // on ')'
        keys(&mut v, "d%");
        assert_eq!(v.buffer.to_string(), "xy");
    }

    #[test]
    fn percent_moves_the_head_in_visual_mode() {
        let mut v = view("(abc)");
        keys(&mut v, "v%"); // Visual mode, head follows % to the ')'
        assert_eq!(v.vim.mode, Mode::Visual);
        assert_eq!(v.cursor, Point::new(0, 4));
        assert_eq!(v.anchor, Some(Point::new(0, 0)));
    }

    // --- vim-parity fixes (see tools/vim-parity) --------------------------

    #[test]
    fn d_g_deletes_linewise_to_the_last_and_first_line() {
        let mut v = view("aa\nbb\ncc");
        keys(&mut v, "jdG"); // delete from line 1 to the last line
        assert_eq!(v.buffer.to_string(), "aa");
        let mut v = view("aa\nbb\ncc");
        keys(&mut v, "jjdgg"); // delete from the last line up to the first
        assert_eq!(v.buffer.to_string(), "");
    }

    #[test]
    fn y_g_yanks_lines_without_moving_the_caret_down() {
        let mut v = view("aa\nbb");
        keys(&mut v, "yyp"); // seed the register comparison; then:
        let mut v = view("  aa\nbb");
        keys(&mut v, "yG"); // downward yank keeps the caret put
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn dollar_takes_a_count_across_lines() {
        let mut v = view("a\n    world");
        keys(&mut v, "2$"); // end of the line one row down
        assert_eq!(v.cursor, Point::new(1, 8));
        let mut v = view("ab\ncd\nef");
        keys(&mut v, "d2$"); // delete through the end of the next line
        assert_eq!(v.buffer.to_string(), "ef");
    }

    #[test]
    fn paste_takes_a_count() {
        let mut v = view("hi\nyo");
        keys(&mut v, "yy2p"); // two copies of the yanked line
        assert_eq!(v.buffer.to_string(), "hi\nhi\nhi\nyo");
        let mut v = view("abcdef");
        keys(&mut v, "yl3p"); // three copies of the yanked char
        assert_eq!(v.buffer.to_string(), "aaaabcdef");
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn linewise_delete_lands_on_first_non_blank() {
        let mut v = view("a\n    cd");
        keys(&mut v, "dd");
        assert_eq!(v.cursor, Point::new(0, 4));
    }

    #[test]
    fn out_of_range_linewise_motion_cancels_the_operator() {
        let mut v = view("only one line");
        keys(&mut v, "d5k"); // can't move up at all -> no-op
        assert_eq!(v.buffer.to_string(), "only one line");
        let mut v = view("aa\nbb\ncc\ndd");
        keys(&mut v, "d5j"); // moves partway (to the last line) -> deletes all
        assert_eq!(v.buffer.to_string(), "");
    }

    #[test]
    fn count_dd_past_the_end_is_a_noop() {
        let mut v = view("only one line");
        keys(&mut v, "2dd"); // 2dd == d1j, which can't move -> no-op
        assert_eq!(v.buffer.to_string(), "only one line");
        let mut v = view("aa\nbb\ncc");
        keys(&mut v, "5dd"); // 5dd on 3 lines deletes what it can
        assert_eq!(v.buffer.to_string(), "");
    }

    #[test]
    fn visual_operates_on_the_char_under_the_cursor() {
        // `vb` at column 0 can't extend, but the anchored char is still selected.
        let mut v = view("xy");
        keys(&mut v, "vk~"); // toggle-case the one selected char
        assert_eq!(v.buffer.to_string(), "Xy");
        let mut v = view("baz");
        keys(&mut v, "vbd");
        assert_eq!(v.buffer.to_string(), "az");
    }

    #[test]
    fn linewise_visual_delete_lands_on_first_non_blank() {
        let mut v = view("xx\n  bb");
        keys(&mut v, "Vld");
        assert_eq!(v.buffer.to_string(), "  bb");
        assert_eq!(v.cursor, Point::new(0, 2));
    }

    #[test]
    fn d_percent_with_no_bracket_is_a_noop() {
        let mut v = view("x xy");
        keys(&mut v, "d%"); // no bracket on the line -> operator cancels
        assert_eq!(v.buffer.to_string(), "x xy");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    // --- char find: f / F / t / T, ; / , -------------------------------------

    #[test]
    fn f_moves_to_next_occurrence_on_the_line() {
        let mut v = view("abcabc");
        keys(&mut v, "fc");
        assert_eq!(v.cursor, Point::new(0, 2));
    }

    #[test]
    fn f_with_no_match_does_not_move() {
        let mut v = view("abcdef");
        keys(&mut v, "fz");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn f_never_crosses_lines() {
        let mut v = view("abc\nxyz");
        keys(&mut v, "fx"); // x only exists on the next line
        assert_eq!(v.cursor, Point::new(0, 0));
        assert_eq!(v.buffer.to_string(), "abc\nxyz"); // and nothing was deleted
    }

    #[test]
    fn count_f_finds_nth_occurrence() {
        let mut v = view("axbxcx");
        keys(&mut v, "2fx");
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn count_f_overshoot_fails_entirely() {
        let mut v = view("axbx");
        keys(&mut v, "3fx"); // only two x's -> no move at all
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn cap_f_moves_backward() {
        let mut v = view("abcabc");
        keys(&mut v, "$Fa"); // from the last char, back to the second 'a'
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn t_stops_just_before_the_target() {
        let mut v = view("abcx");
        keys(&mut v, "tx");
        assert_eq!(v.cursor, Point::new(0, 2));
    }

    #[test]
    fn t_on_adjacent_char_does_not_move() {
        let mut v = view("axb");
        keys(&mut v, "tx"); // landing spot == cursor
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn cap_t_stops_just_after_the_target_backward() {
        let mut v = view("xabc");
        keys(&mut v, "$Tx"); // from 'c' back to just after the 'x'
        assert_eq!(v.cursor, Point::new(0, 1));
    }

    #[test]
    fn semicolon_repeats_the_last_find() {
        let mut v = view("axbxcx");
        keys(&mut v, "fx;");
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn semicolon_takes_a_count() {
        let mut v = view("axbxcx");
        keys(&mut v, "fx2;");
        assert_eq!(v.cursor, Point::new(0, 5));
    }

    #[test]
    fn comma_reverses_the_last_find() {
        let mut v = view("axbxcx");
        keys(&mut v, "fx;;"); // -> col 5
        assert_eq!(v.cursor, Point::new(0, 5));
        keys(&mut v, ","); // reversed: back to col 3
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn semicolon_after_t_skips_the_adjacent_char_stall() {
        let mut v = view("abxcx");
        keys(&mut v, "tx"); // just before the first x
        assert_eq!(v.cursor, Point::new(0, 1));
        keys(&mut v, ";"); // would stall at col 1; vim skips ahead
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn comma_after_t_reverses_with_till_semantics() {
        let mut v = view("x..x..x");
        keys(&mut v, "tx"); // just before the middle x
        assert_eq!(v.cursor, Point::new(0, 2));
        keys(&mut v, ","); // reversed T: just after the first x
        assert_eq!(v.cursor, Point::new(0, 1));
        keys(&mut v, ";"); // forward t again
        assert_eq!(v.cursor, Point::new(0, 2));
    }

    #[test]
    fn d_f_deletes_through_the_target_inclusive() {
        let mut v = view("abcxdef");
        keys(&mut v, "dfx");
        assert_eq!(v.buffer.to_string(), "def");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn d_t_deletes_up_to_but_not_the_target() {
        let mut v = view("abcxdef");
        keys(&mut v, "dtx");
        assert_eq!(v.buffer.to_string(), "xdef");
    }

    #[test]
    fn d_cap_f_deletes_backward_exclusive() {
        let mut v = view("abcxdef");
        keys(&mut v, "$dFx"); // from 'f': deletes "xde", keeps the x's replacement start
        assert_eq!(v.buffer.to_string(), "abcf");
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn d_cap_t_deletes_backward_up_to_after_the_target() {
        let mut v = view("abcxdef");
        keys(&mut v, "$dTx"); // deletes "de", keeps the x
        assert_eq!(v.buffer.to_string(), "abcxf");
    }

    #[test]
    fn c_f_changes_through_the_target() {
        let mut v = view("abcxdef");
        keys(&mut v, "cfx");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.buffer.to_string(), "def");
        keys(&mut v, "Z"); // typed in insert mode
        assert_eq!(v.buffer.to_string(), "Zdef");
    }

    #[test]
    fn y_f_yanks_through_the_target() {
        let mut v = view("abcxdef");
        keys(&mut v, "yfxp"); // yank "abcx" (cursor stays on 'a'), paste after
        assert_eq!(v.buffer.to_string(), "aabcxbcxdef");
    }

    #[test]
    fn failed_find_cancels_a_pending_operator_without_editing() {
        let mut v = view("abcdef");
        keys(&mut v, "dfz"); // no z on the line -> no edit
        assert_eq!(v.buffer.to_string(), "abcdef");
        assert_eq!(v.cursor, Point::new(0, 0));
        keys(&mut v, "x"); // operator is gone: x deletes one char normally
        assert_eq!(v.buffer.to_string(), "bcdef");
    }

    #[test]
    fn count_find_overshoot_cancels_a_pending_operator() {
        let mut v = view("axbx");
        keys(&mut v, "d3fx"); // only two x's -> whole edit cancels
        assert_eq!(v.buffer.to_string(), "axbx");
    }

    #[test]
    fn visual_f_extends_the_selection() {
        let mut v = view("abcxdef");
        keys(&mut v, "vfx");
        assert_eq!(v.vim.mode, Mode::Visual);
        assert_eq!(v.anchor, Some(Point::new(0, 0)));
        assert_eq!(v.cursor, Point::new(0, 3));
        keys(&mut v, "yp"); // yank "abcx", paste after the caret on 'a'
        assert_eq!(v.buffer.to_string(), "aabcxbcxdef");
    }

    #[test]
    fn visual_semicolon_extends_the_selection_further() {
        let mut v = view("axbxc");
        keys(&mut v, "vfx;");
        assert_eq!(v.vim.mode, Mode::Visual);
        assert_eq!(v.anchor, Some(Point::new(0, 0)));
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn escape_while_waiting_for_the_find_char_cancels() {
        let mut v = view("abc");
        keys(&mut v, "f");
        esc(&mut v);
        keys(&mut v, "x"); // no stray find: x deletes a char normally
        assert_eq!(v.buffer.to_string(), "bc");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn non_char_key_while_waiting_for_the_find_char_cancels() {
        let mut v = view("abx");
        keys(&mut v, "f");
        key(&mut v, Key::Right); // swallowed, cancels the find
        assert_eq!(v.cursor, Point::new(0, 0));
        keys(&mut v, "x");
        assert_eq!(v.buffer.to_string(), "bx");
    }

    #[test]
    fn last_find_survives_unrelated_commands() {
        let mut v = view("axbxcx\nqxrx");
        keys(&mut v, "fx"); // -> (0, 1)
        keys(&mut v, "dd"); // unrelated edit clears pending state, not last-find
        assert_eq!(v.buffer.to_string(), "qxrx");
        keys(&mut v, ";");
        assert_eq!(v.cursor, Point::new(0, 1));
    }

    // --- paragraph motions `{` / `}` -----------------------------------------

    /// Two paragraphs separated by blank lines:
    /// 0 "aaa", 1 "bbb", 2 "", 3 "ccc", 4 "ddd", 5 "", 6 "eee".
    fn para_view() -> EditorView {
        view("aaa\nbbb\n\nccc\nddd\n\neee")
    }

    #[test]
    fn brace_forward_moves_to_the_blank_line_after_the_paragraph() {
        let mut v = para_view();
        keys(&mut v, "}");
        assert_eq!(v.cursor, Point::new(2, 0));
    }

    #[test]
    fn brace_forward_from_a_blank_line_skips_past_the_next_paragraph() {
        let mut v = para_view();
        v.cursor = Point::new(2, 0);
        keys(&mut v, "}");
        assert_eq!(v.cursor, Point::new(5, 0));
    }

    #[test]
    fn brace_forward_with_no_blank_below_lands_at_end_of_buffer() {
        // No blank line follows: land on the last line, at its end (vim's `}`
        // at EOF); normal-mode clamping pulls the caret onto the last char.
        let mut v = view("aaa\nbbb");
        keys(&mut v, "}");
        assert_eq!(v.cursor, Point::new(1, 2));
    }

    #[test]
    fn brace_backward_moves_to_the_blank_line_before_the_paragraph() {
        let mut v = para_view();
        v.cursor = Point::new(4, 1);
        keys(&mut v, "{");
        assert_eq!(v.cursor, Point::new(2, 0));
    }

    #[test]
    fn brace_backward_from_a_blank_line_skips_past_the_previous_paragraph() {
        let mut v = para_view();
        v.cursor = Point::new(5, 0);
        keys(&mut v, "{");
        assert_eq!(v.cursor, Point::new(2, 0));
    }

    #[test]
    fn brace_backward_with_no_blank_above_lands_at_the_top() {
        let mut v = para_view();
        v.cursor = Point::new(1, 2);
        keys(&mut v, "{");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn counted_brace_forward_skips_two_paragraph_boundaries() {
        let mut v = para_view();
        keys(&mut v, "2}");
        assert_eq!(v.cursor, Point::new(5, 0));
    }

    #[test]
    fn d_brace_deletes_the_rest_of_the_paragraph_but_not_the_blank_line() {
        // Exclusive charwise span [cursor, blank-line-start): the blank line
        // itself survives.
        let mut v = view("aaa\nbbb\n\nccc");
        keys(&mut v, "d}");
        assert_eq!(v.buffer.to_string(), "\nccc");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn y_brace_then_paste_round_trips_the_paragraph_text() {
        let mut v = view("aaa\nbbb\n\nccc");
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "y}");
        assert_eq!(clip.get().as_deref(), Some("aaa\nbbb\n"));
        assert_eq!(v.buffer.to_string(), "aaa\nbbb\n\nccc"); // yank leaves text alone
        keys_clip(&mut v, &mut clip, "P"); // charwise paste before (0,0)
        assert_eq!(v.buffer.to_string(), "aaa\nbbb\naaa\nbbb\n\nccc");
    }

    #[test]
    fn visual_line_brace_forward_extends_the_selection_through_the_paragraph() {
        let mut v = para_view();
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "V}");
        assert_eq!(v.vim.mode, Mode::VisualLine);
        assert_eq!(v.anchor, Some(Point::new(0, 0)));
        assert_eq!(v.cursor, Point::new(2, 0));
        keys_clip(&mut v, &mut clip, "y"); // linewise yank of lines 0..=2
        assert_eq!(clip.get().as_deref(), Some("aaa\nbbb\n"));
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn whitespace_only_line_is_not_a_paragraph_boundary() {
        // Line 1 contains spaces, so it is NOT blank (vim: only a truly empty
        // line separates paragraphs); the first real boundary is line 3.
        let mut v = view("aaa\n   \nbbb\n\nccc");
        keys(&mut v, "}");
        assert_eq!(v.cursor, Point::new(3, 0));
    }

    // --- text objects (`iw`/`aw`, pairs, quotes) ------------------------------

    #[test]
    fn diw_deletes_the_word_under_the_cursor() {
        let mut v = view("one two three");
        v.cursor = Point::new(0, 5); // mid-"two"
        keys(&mut v, "diw");
        assert_eq!(v.buffer.to_string(), "one  three");
        assert_eq!(v.cursor, Point::new(0, 4));
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn diw_on_whitespace_deletes_the_whitespace_run() {
        let mut v = view("one   two");
        v.cursor = Point::new(0, 4); // middle of the space run
        keys(&mut v, "diw");
        assert_eq!(v.buffer.to_string(), "onetwo");
        assert_eq!(v.cursor, Point::new(0, 3));
    }

    #[test]
    fn daw_deletes_the_word_and_its_trailing_space() {
        let mut v = view("one two three");
        v.cursor = Point::new(0, 5); // mid-"two"
        keys(&mut v, "daw");
        assert_eq!(v.buffer.to_string(), "one three");
    }

    #[test]
    fn daw_at_end_of_line_takes_the_leading_space_instead() {
        let mut v = view("one two");
        v.cursor = Point::new(0, 5); // mid-"two", no trailing whitespace
        keys(&mut v, "daw");
        assert_eq!(v.buffer.to_string(), "one");
    }

    #[test]
    fn d2aw_deletes_two_words_with_their_whitespace() {
        let mut v = view("one two three four");
        v.cursor = Point::new(0, 0); // start of "one"
        keys(&mut v, "d2aw");
        assert_eq!(v.buffer.to_string(), "three four");
    }

    #[test]
    fn d3aw_reaching_end_of_line_stops_cleanly() {
        let mut v = view("one two three");
        v.cursor = Point::new(0, 0);
        keys(&mut v, "d3aw");
        assert_eq!(v.buffer.to_string(), "");
    }

    #[test]
    fn d2iw_takes_the_word_and_the_following_whitespace_run() {
        // `iw` counts each same-class run, so `2iw` on a word is word + the
        // whitespace run after it (not the next word).
        let mut v = view("one two three");
        v.cursor = Point::new(0, 0);
        keys(&mut v, "d2iw");
        assert_eq!(v.buffer.to_string(), "two three");
    }

    #[test]
    fn d3iw_takes_word_space_word() {
        let mut v = view("one two three");
        v.cursor = Point::new(0, 0);
        keys(&mut v, "d3iw");
        assert_eq!(v.buffer.to_string(), " three");
    }

    #[test]
    fn ciw_removes_the_word_and_enters_insert() {
        let mut v = view("one two three");
        v.cursor = Point::new(0, 4); // start of "two"
        keys(&mut v, "ciw");
        assert_eq!(v.buffer.to_string(), "one  three");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.cursor, Point::new(0, 4));
        keys(&mut v, "TWO");
        assert_eq!(v.buffer.to_string(), "one TWO three");
    }

    #[test]
    fn yiw_then_paste_round_trips_the_word() {
        let mut v = view("one two three");
        v.cursor = Point::new(0, 5); // mid-"two"
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "yiw");
        assert_eq!(clip.get().as_deref(), Some("two"));
        assert_eq!(v.buffer.to_string(), "one two three"); // yank leaves text alone
        assert_eq!(v.cursor, Point::new(0, 4)); // cursor at object start
        keys_clip(&mut v, &mut clip, "p");
        assert_eq!(v.buffer.to_string(), "one ttwowo three");
    }

    #[test]
    fn di_paren_deletes_inside_the_parens() {
        let mut v = view("foo(bar, baz) qux");
        v.cursor = Point::new(0, 6); // inside the parens
        keys(&mut v, "di(");
        assert_eq!(v.buffer.to_string(), "foo() qux");
        assert_eq!(v.cursor, Point::new(0, 4)); // on the ')'
    }

    #[test]
    fn di_paren_picks_the_nearest_enclosing_pair_when_nested() {
        let mut v = view("a(b(c)d)e");
        v.cursor = Point::new(0, 4); // on 'c', inside the inner pair
        keys(&mut v, "di(");
        assert_eq!(v.buffer.to_string(), "a(b()d)e");
    }

    #[test]
    fn d2i_paren_steps_out_one_nesting_level() {
        let mut v = view("a(b(c)d)e");
        v.cursor = Point::new(0, 4); // on 'c', inside the inner pair
        keys(&mut v, "d2i(");
        assert_eq!(v.buffer.to_string(), "a()e"); // deletes the outer pair's contents
    }

    #[test]
    fn d2a_paren_steps_out_and_takes_the_delimiters() {
        let mut v = view("a(b(c)d)e");
        v.cursor = Point::new(0, 4);
        keys(&mut v, "d2a(");
        assert_eq!(v.buffer.to_string(), "ae");
    }

    #[test]
    fn count_beyond_available_nesting_is_a_no_op() {
        let mut v = view("a(b)c");
        v.cursor = Point::new(0, 2); // inside the only pair
        keys(&mut v, "d2i(");
        assert_eq!(v.buffer.to_string(), "a(b)c"); // no second level: nothing deleted
    }

    #[test]
    fn di_brace_works_across_lines() {
        let mut v = view("fn f() {\n    body\n}");
        v.cursor = Point::new(1, 5); // inside the braces
        keys(&mut v, "di{");
        assert_eq!(v.buffer.to_string(), "fn f() {}");
    }

    #[test]
    fn di_paren_with_cursor_on_the_opening_paren() {
        let mut v = view("foo(bar) qux");
        v.cursor = Point::new(0, 3); // ON the '('
        keys(&mut v, "di(");
        assert_eq!(v.buffer.to_string(), "foo() qux");
    }

    #[test]
    fn da_paren_includes_the_delimiters() {
        let mut v = view("foo(bar) qux");
        v.cursor = Point::new(0, 5); // inside
        keys(&mut v, "da(");
        assert_eq!(v.buffer.to_string(), "foo qux");
    }

    #[test]
    fn dib_and_di_close_paren_alias_di_open_paren() {
        let mut v = view("a(bc)d");
        v.cursor = Point::new(0, 2);
        keys(&mut v, "dib");
        assert_eq!(v.buffer.to_string(), "a()d");

        let mut v = view("a(bc)d");
        v.cursor = Point::new(0, 2);
        keys(&mut v, "di)");
        assert_eq!(v.buffer.to_string(), "a()d");
    }

    #[test]
    fn di_brace_alias_capital_b_and_close_brace() {
        let mut v = view("a{bc}d");
        v.cursor = Point::new(0, 2);
        keys(&mut v, "diB");
        assert_eq!(v.buffer.to_string(), "a{}d");

        let mut v = view("a{bc}d");
        v.cursor = Point::new(0, 2);
        keys(&mut v, "di}");
        assert_eq!(v.buffer.to_string(), "a{}d");
    }

    #[test]
    fn di_bracket_via_open_and_close_chars() {
        let mut v = view("a[bc]d");
        v.cursor = Point::new(0, 2);
        keys(&mut v, "di[");
        assert_eq!(v.buffer.to_string(), "a[]d");

        let mut v = view("a[bc]d");
        v.cursor = Point::new(0, 2);
        keys(&mut v, "di]");
        assert_eq!(v.buffer.to_string(), "a[]d");
    }

    #[test]
    fn di_angle_deletes_inside_angle_brackets() {
        let mut v = view("Vec<String> x");
        v.cursor = Point::new(0, 6); // inside <>
        keys(&mut v, "di<");
        assert_eq!(v.buffer.to_string(), "Vec<> x");
    }

    #[test]
    fn di_paren_with_no_enclosing_pair_cancels_the_operator() {
        let mut v = view("no pairs here");
        v.cursor = Point::new(0, 3);
        keys(&mut v, "di(");
        assert_eq!(v.buffer.to_string(), "no pairs here");
        assert_eq!(v.vim.mode, Mode::Normal);
        // The operator was dropped: a following 'w' is a plain motion, not `dw`.
        keys(&mut v, "w");
        assert_eq!(v.buffer.to_string(), "no pairs here");
        assert_eq!(v.cursor, Point::new(0, 9));
    }

    #[test]
    fn di_quote_deletes_inside_the_quotes() {
        let mut v = view("say \"hello there\" ok");
        v.cursor = Point::new(0, 7); // inside the quotes
        keys(&mut v, "di\"");
        assert_eq!(v.buffer.to_string(), "say \"\" ok");
    }

    #[test]
    fn di_quote_before_the_quotes_seeks_forward_on_the_line() {
        let mut v = view("say \"hello\" ok");
        v.cursor = Point::new(0, 1); // before the quoted span
        keys(&mut v, "di\"");
        assert_eq!(v.buffer.to_string(), "say \"\" ok");
    }

    #[test]
    fn ci_single_quote_enters_insert_inside_the_quotes() {
        let mut v = view("x = 'old' y");
        v.cursor = Point::new(0, 6);
        keys(&mut v, "ci'");
        assert_eq!(v.buffer.to_string(), "x = '' y");
        assert_eq!(v.vim.mode, Mode::Insert);
        keys(&mut v, "new");
        assert_eq!(v.buffer.to_string(), "x = 'new' y");
    }

    #[test]
    fn ya_backtick_yanks_including_the_backticks_and_trailing_space() {
        let mut v = view("run `ls -la` now");
        v.cursor = Point::new(0, 7);
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "ya`");
        // `a` grabs the delimiters plus the trailing whitespace (vim).
        assert_eq!(clip.get().as_deref(), Some("`ls -la` "));
        assert_eq!(v.buffer.to_string(), "run `ls -la` now");
    }

    #[test]
    fn di_quote_ignores_backslash_escaped_quotes() {
        // The inner `\"` is escaped, so the string spans the outer quotes;
        // `di"` empties the whole thing rather than stopping at the escape.
        let mut v = view("x = \"a\\\"b\" y");
        v.cursor = Point::new(0, 6); // inside the string
        keys(&mut v, "di\"");
        assert_eq!(v.buffer.to_string(), "x = \"\" y");
    }

    #[test]
    fn even_backslashes_before_a_quote_do_not_escape_it() {
        // `\\` is a literal backslash, so the following `"` still closes.
        let mut v = view("x = \"a\\\\\" y");
        v.cursor = Point::new(0, 6);
        keys(&mut v, "di\"");
        assert_eq!(v.buffer.to_string(), "x = \"\" y");
    }

    #[test]
    fn di_quote_with_no_quotes_on_the_line_is_a_no_op() {
        let mut v = view("nothing quoted");
        v.cursor = Point::new(0, 3);
        keys(&mut v, "di\"");
        assert_eq!(v.buffer.to_string(), "nothing quoted");
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn visual_iw_reshapes_the_selection_to_the_word() {
        let mut v = view("one two three");
        v.cursor = Point::new(0, 5); // mid-"two"
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "viw");
        assert_eq!(v.vim.mode, Mode::Visual);
        assert_eq!(v.anchor, Some(Point::new(0, 4)));
        assert_eq!(v.cursor, Point::new(0, 6)); // inclusive last char of "two"
        keys_clip(&mut v, &mut clip, "y");
        assert_eq!(clip.get().as_deref(), Some("two"));
    }

    #[test]
    fn visual_i_paren_reshapes_to_the_pair_contents() {
        let mut v = view("foo(bar) qux");
        v.cursor = Point::new(0, 5);
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "vi(");
        assert_eq!(v.vim.mode, Mode::Visual);
        assert_eq!(v.anchor, Some(Point::new(0, 4)));
        assert_eq!(v.cursor, Point::new(0, 6));
        keys_clip(&mut v, &mut clip, "d");
        assert_eq!(v.buffer.to_string(), "foo() qux");
    }

    #[test]
    fn visual_a_quote_includes_the_quotes_and_trailing_space() {
        let mut v = view("say \"hi\" ok");
        v.cursor = Point::new(0, 5);
        let mut clip = InMemoryClipboard::default();
        keys_clip(&mut v, &mut clip, "va\"y");
        // The quotes plus the trailing whitespace after the closer (vim `a"`).
        assert_eq!(clip.get().as_deref(), Some("\"hi\" "));
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn di_paren_on_empty_parens_changes_nothing() {
        let mut v = view("foo() bar");
        v.cursor = Point::new(0, 3); // on '('
        keys(&mut v, "di(");
        assert_eq!(v.buffer.to_string(), "foo() bar");
        assert_eq!(v.vim.mode, Mode::Normal);
    }

    #[test]
    fn ci_paren_on_empty_parens_enters_insert_between_them() {
        let mut v = view("foo() bar");
        v.cursor = Point::new(0, 3); // on '('
        keys(&mut v, "ci(");
        assert_eq!(v.buffer.to_string(), "foo() bar");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.cursor, Point::new(0, 4)); // between the parens
        keys(&mut v, "x");
        assert_eq!(v.buffer.to_string(), "foo(x) bar");
    }

    #[test]
    fn i_without_an_operator_still_enters_insert() {
        let mut v = view("abc");
        v.cursor = Point::new(0, 1);
        keys(&mut v, "iX");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.buffer.to_string(), "aXbc");
    }

    #[test]
    fn a_without_an_operator_still_appends() {
        let mut v = view("abc");
        v.cursor = Point::new(0, 1);
        keys(&mut v, "aX");
        assert_eq!(v.vim.mode, Mode::Insert);
        assert_eq!(v.buffer.to_string(), "abXc");
    }

    #[test]
    fn visual_i_then_invalid_object_key_cancels_cleanly() {
        let mut v = view("one two");
        v.cursor = Point::new(0, 1);
        keys(&mut v, "vi");
        assert_eq!(v.vim.mode, Mode::Visual); // waiting for the object kind
        keys(&mut v, "z");
        // Invalid object: selection intact, still Visual, nothing edited.
        assert_eq!(v.vim.mode, Mode::Visual);
        assert_eq!(v.anchor, Some(Point::new(0, 1)));
        assert_eq!(v.cursor, Point::new(0, 1));
        assert_eq!(v.buffer.to_string(), "one two");
        // And the parser is clean: a motion afterwards just moves.
        keys(&mut v, "l");
        assert_eq!(v.cursor, Point::new(0, 2));
        assert_eq!(v.vim.mode, Mode::Visual);
    }

    #[test]
    fn d_then_i_then_invalid_object_key_cancels_the_operator() {
        let mut v = view("one two");
        keys(&mut v, "diz");
        assert_eq!(v.buffer.to_string(), "one two");
        assert_eq!(v.vim.mode, Mode::Normal);
        // Operator gone: 'w' afterwards is a plain motion.
        keys(&mut v, "w");
        assert_eq!(v.buffer.to_string(), "one two");
        assert_eq!(v.cursor, Point::new(0, 4));
    }

    // --- editable projections ------------------------------------------------
    //
    // A view whose content is a *projection* of other documents folds its edits
    // back into them (`garden_core::projection`). What matters here is that the
    // ordinary vim commands do it: none of them knows a projection exists, so
    // these tests are really about the mutation choke point reporting each edit
    // as the right line splice.

    use garden_core::projection::{ChromeRole, Decor, LineOrigin, NewLine, Projection, Span};

    /// A one-hunk unified diff over `a.txt`, whose base is `one / two / three`
    /// and whose working tree is `one / TWO / three`:
    ///
    /// ```text
    ///  0  @@ -1,3 +1,3 @@   hunk header (a span header)
    ///  1   one              context
    ///  2  -two              deletion
    ///  3  +TWO              addition
    ///  4   three            context
    /// ```
    fn projected_diff() -> EditorView {
        let mut v = view(" @@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three");
        v.buffer = Buffer::from_str("@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three");
        let decor = Decor {
            same: (" ".into(), String::new()),
            added: ("+".into(), "added".into()),
            removed: ("-".into(), "removed".into()),
            new_line: NewLine::DiffMarker,
            gutter: false,
        };
        let mut proj = Projection::new(
            vec!["a.txt".into()],
            vec![Span {
                source: 0,
                target: (0, 3),
                group: Some(0),
            }],
            decor,
        );
        proj.push(
            LineOrigin::Chrome {
                role: ChromeRole::SpanHeader,
                locked: false,
            },
            "hunk",
            Some(0),
        );
        proj.push(LineOrigin::Live { added: false }, "", Some(0));
        proj.push(LineOrigin::Ghost { text: "two".into() }, "removed", Some(0));
        proj.push(LineOrigin::Live { added: true }, "added", Some(0));
        proj.push(LineOrigin::Live { added: false }, "", Some(0));
        v.projection = Some(proj);
        v
    }

    /// What `a.txt` would be written as, folding the view's current state back.
    fn folded(v: &EditorView) -> Vec<String> {
        let lines: Vec<String> = (0..v.buffer.line_count())
            .map(|i| v.buffer.line(i))
            .collect();
        let edits = v.projection.as_ref().unwrap().resolve(&lines);
        edits[0].lines.clone()
    }

    #[test]
    fn projection_dd_on_an_added_line_drops_the_addition() {
        let mut v = projected_diff();
        keys(&mut v, "4Gdd"); // onto `+TWO`, delete it
        assert_eq!(folded(&v), ["one", "three"]);
    }

    #[test]
    fn projection_dd_on_a_removed_line_reverts_the_deletion() {
        let mut v = projected_diff();
        keys(&mut v, "3Gdd"); // onto `-two`; deleting it puts the base line back
        assert_eq!(folded(&v), ["one", "two", "TWO", "three"]);
    }

    #[test]
    fn projection_dd_on_the_hunk_header_reverts_the_hunk() {
        let mut v = projected_diff();
        keys(&mut v, "ggdd");
        // The addition is gone and the deletion is back as context: the file
        // now matches the base exactly.
        assert_eq!(folded(&v), ["one", "two", "three"]);
        assert_eq!(v.buffer.to_string(), "@@ -1,3 +1,3 @@\n one\n two\n three");
    }

    #[test]
    fn projection_undo_restores_origins_not_just_text() {
        let mut v = projected_diff();
        keys(&mut v, "4Gdd");
        keys(&mut v, "u");
        // Back to the projected text…
        assert_eq!(
            v.buffer.to_string(),
            "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three"
        );
        // …and back to the projected *meaning*: the fold is the original again.
        assert_eq!(folded(&v), ["one", "TWO", "three"]);
    }

    #[test]
    fn projection_insert_mode_typing_lands_in_the_right_span() {
        let mut v = projected_diff();
        keys(&mut v, "GOinserted");
        handle(
            &mut v,
            Key::Escape,
            50,
            WIDE,
            &mut InMemoryClipboard::default(),
        );
        assert_eq!(folded(&v), ["one", "TWO", "inserted", "three"]);
    }

    #[test]
    fn projection_visual_line_delete_resolves_every_line_it_covers() {
        let mut v = projected_diff();
        keys(&mut v, "3GVjd"); // the deletion and the addition together
        assert_eq!(folded(&v), ["one", "two", "three"]);
    }

    #[test]
    fn projection_styles_follow_their_lines_through_an_insertion() {
        let mut v = projected_diff();
        let styles = |v: &EditorView| v.projection.as_ref().unwrap().line_styles();
        assert_eq!(styles(&v), ["hunk", "", "removed", "added", ""]);
        keys(&mut v, "ggo");
        handle(
            &mut v,
            Key::Escape,
            50,
            WIDE,
            &mut InMemoryClipboard::default(),
        );
        assert_eq!(styles(&v), ["hunk", "", "", "removed", "added", ""]);
    }
}
