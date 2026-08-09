//! The vim-style command line (`:`) and its ex commands, plus the `/` and `?`
//! search prompts which share the same status-bar input mechanism. Opened
//! from Normal mode with `:`, `/`, or `?`, it collects a line of text; on
//! Enter a `:` line parses into a [`Command`] while a search line's text is
//! the search pattern.

/// What an open command line is collecting: an ex command or a search pattern.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CommandLineKind {
    #[default]
    Command,
    SearchForward,
    SearchBackward,
}

/// An in-progress command line: the text typed after the leading `:`, `/`,
/// or `?`.
#[derive(Default)]
pub struct CommandLine {
    pub kind: CommandLineKind,
    pub input: String,
}

impl CommandLine {
    pub fn new() -> CommandLine {
        CommandLine::default()
    }

    /// A `:` command line pre-filled with `input` (e.g. `"w "` for the
    /// save-as filename prompt), so the user completes rather than types it all.
    pub fn with_input(input: impl Into<String>) -> CommandLine {
        CommandLine {
            kind: CommandLineKind::Command,
            input: input.into(),
        }
    }

    /// A `/` (forward) or `?` (backward) search prompt.
    pub fn new_search(forward: bool) -> CommandLine {
        CommandLine {
            kind: if forward {
                CommandLineKind::SearchForward
            } else {
                CommandLineKind::SearchBackward
            },
            input: String::new(),
        }
    }

    /// The prompt character shown before the input.
    pub fn prefix(&self) -> char {
        match self.kind {
            CommandLineKind::Command => ':',
            CommandLineKind::SearchForward => '/',
            CommandLineKind::SearchBackward => '?',
        }
    }

    /// The line as displayed (and as exposed in the debug `/state` JSON):
    /// prompt character plus input.
    pub fn display(&self) -> String {
        format!("{}{}", self.prefix(), self.input)
    }

    pub fn push(&mut self, c: char) {
        self.input.push(c);
    }

    /// Remove the last character. Returns `false` when the line was already
    /// empty (the caller closes the command line in that case, matching vim).
    pub fn backspace(&mut self) -> bool {
        self.input.pop().is_some()
    }

    pub fn command(&self) -> Command {
        parse(&self.input)
    }
}

/// A parsed ex command.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// `:e <file>` — open a file in the focused pane.
    Edit(String),
    /// `:E` / `:Explore` — open the directory browser in the focused pane,
    /// listing the directory of the focused file (vim's `:Explore`).
    Explore,
    /// `:Git` — open the git history browser in the focused pane, rooted at the
    /// focused file's directory.
    Git,
    /// `:Diff [--stat] [rev]` — review a diff in the focused pane, in the
    /// `garden-diff` GPP client: an editable before/after split (base → working
    /// tree, right side writes back on `^S`) plus read-only unified and per-file
    /// stat views. `rev` is the base ref; empty auto-detects the upstream or
    /// `main`/`master`. `--stat` opens straight into the summary diagram.
    Diff { rev: String, stat: bool },
    /// `:Review [base]` / `:Pr [base]` — open the `garden-diff` editable review
    /// (base → working tree) in the focused pane; an empty `base` auto-detects the
    /// upstream or `main`/`master`. An alias of `:Diff` without `--stat`.
    Review(String),
    /// `:Review2 [base]` / `:ReviewSplit` — also the `garden-diff` editable
    /// review (the before/after split is its default mode). Kept as an alias for
    /// muscle memory; identical to `:Review`.
    ReviewSplit(String),
    /// `:PR [number]` — open `garden-diff` in PR mode in the focused pane: the
    /// PR's base merge-base → working tree, editable, with the PR
    /// number/title/author/description, the conversation, and the inline review
    /// comments (threaded into the unified view). Resolved via `gh`; the PR's
    /// head branch must be checked out (else a `gh pr checkout` hint). An empty
    /// argument reviews the current branch's PR — the same thing `garden pr` opens.
    Pr(String),
    /// `:w` — write the focused buffer.
    Write,
    /// `:w <path>` — write the focused buffer to `path`, adopting it as the
    /// pane's file (vim's save-as). Also how a save-protected buffer (e.g. the
    /// Petal-IDE scratch) is saved without overwriting the source.
    WriteAs(String),
    /// `:q` — quit.
    Quit,
    /// `:wq` / `:x` — write then quit.
    WriteQuit,
    /// `:wa` — write every dirty pane that has a file path.
    WriteAll,
    /// `:wqa` / `:xa` — write all, then quit.
    WriteAllQuit,
    /// `:windownew` — open a new OS window (the windowed frontend fulfills the
    /// intent; see `App::take_new_window_request`). Deliberately *not* `:new`:
    /// vim reserves `:new` for split-with-empty-buffer.
    WindowNew,
    /// `:noh` / `:nohlsearch` — clear search match highlights (the pattern
    /// stays remembered for `n`/`N`).
    NoHighlight,
    /// `:report <text>` — file a bug/feature report, capturing the recent event
    /// log as context (see [`crate::event_log`]). Carries the description, which
    /// may be empty.
    Report(String),
    /// `:s/pat/rep/[flags]` (current line), `:%s/...` (whole buffer), or
    /// `:N,Ms/...` (a line range) — plain-text search-and-replace. Flags:
    /// `g` (all matches on a line), `i`/`I` (ignore / force case).
    Substitute(Substitution),
    /// `:set wrap` / `:set nowrap` (also the bare `:wrap` / `:nowrap`) — turn
    /// soft line-wrapping on or off for the focused editor pane.
    SetWrap(bool),
    /// `:State` — toggle the Petal-IDE live-state inspector overlay on panel
    /// panes (every value the last frame bound, by name, + frame count).
    ToggleState,
    /// `:back` — step back one entry in the focused panel's browser-style
    /// history (the keyboard counterpart is `Ctrl+[`). Walks the panel's unified
    /// history stack: through any in-script navigations, then to the previously
    /// visited `.ptl` screen. A no-op at the start of history.
    Back,
    /// `:forward` — step forward one entry in the focused panel's history, undoing
    /// a `:back` (the keyboard counterpart is `Ctrl+]`). A no-op at the end.
    Forward,
    /// A bare line address — `:42`, `:$` (also `:%`, whose range means "through
    /// the last line") — vim's jump-to-line. Clamped to the buffer when run;
    /// the cursor lands on the line's first non-blank column.
    Goto(Addr),
    /// An empty line (no-op).
    Empty,
    /// Anything unrecognized; carries the raw text for an error message.
    Unknown(String),
}

/// A line address in a `:s` range prefix: an absolute 1-based line number, the
/// cursor's line (`.`), or the buffer's last line (`$`). Resolved against the
/// focused pane when the substitution runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addr {
    Line(usize),
    Current,
    Last,
}

/// A parsed `:s` / `:%s` / `:N,Ms` substitution. Pattern and replacement are
/// literal text (no regex). An empty `pattern` means "reuse the last search
/// pattern" — the App resolves that against the focused pane.
#[derive(Debug, PartialEq, Eq)]
pub struct Substitution {
    /// `true` for `:%s` (every line); `false` for `:s` (the current line or
    /// the explicit `range`).
    pub whole_buffer: bool,
    /// An explicit line range (`:1,5s`, `:.,$s`, or a single `:5s`), 1-based
    /// and inclusive; `None` falls back to `whole_buffer` / the cursor's line.
    pub range: Option<(Addr, Addr)>,
    pub pattern: String,
    pub replacement: String,
    /// The `g` flag: replace every occurrence on a line, not just the first.
    pub global: bool,
    /// The `i` flag: match case-insensitively. `I` (force case-sensitive)
    /// is accepted and wins over `i`; the default is already case-sensitive.
    pub ignore_case: bool,
}

/// Split a leading `--stat` flag off a `:Diff` argument, returning
/// `(stat, rev)`. `--stat` may stand alone or precede a revision
/// (`--stat HEAD~2..HEAD`); anything else is treated as the revision.
fn parse_stat_flag(arg: &str) -> (bool, String) {
    let mut parts = arg.splitn(2, char::is_whitespace);
    match parts.next() {
        Some("--stat") => (true, parts.next().unwrap_or("").trim().to_string()),
        _ => (false, arg.to_string()),
    }
}

/// Parse the text typed after the leading `:`.
pub fn parse(input: &str) -> Command {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Command::Empty;
    }
    // Substitution (`:s.../:%s...`) is matched before the whitespace split,
    // since `s/a/b/` carries no whitespace and the delimiter glues it together.
    if let Some(cmd) = parse_substitute(trimmed) {
        return cmd;
    }
    // A bare address (`:42`, `:$`, `:%`, `:1,5`) with no command after it is
    // vim's jump-to-line. Tried after substitution so `:5s/a/b/` stays a
    // substitution.
    if let Some(cmd) = parse_goto(trimmed) {
        return cmd;
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match verb {
        "w" | "write" => {
            if arg.is_empty() {
                Command::Write
            } else {
                Command::WriteAs(arg.to_string())
            }
        }
        "q" | "quit" => Command::Quit,
        "wq" | "x" => Command::WriteQuit,
        "wa" | "wall" => Command::WriteAll,
        "wqa" | "xa" | "wqall" | "xall" => Command::WriteAllQuit,
        "windownew" => Command::WindowNew,
        "e" | "edit" => Command::Edit(arg.to_string()),
        "E" | "Explore" | "Ex" => Command::Explore,
        "Git" => Command::Git,
        "Diff" | "D" => {
            let (stat, rev) = parse_stat_flag(arg);
            Command::Diff { rev, stat }
        }
        "Review" | "Pr" => Command::Review(arg.to_string()),
        "Review2" | "ReviewSplit" | "Reviews" => Command::ReviewSplit(arg.to_string()),
        "PR" => Command::Pr(arg.to_string()),
        "State" => Command::ToggleState,
        "back" => Command::Back,
        "forward" | "fwd" => Command::Forward,
        "report" => Command::Report(arg.to_string()),
        "noh" | "nohl" | "nohlsearch" => Command::NoHighlight,
        // `:set wrap` / `:set nowrap`, plus the bare `:wrap` / `:nowrap`.
        "set" => match arg {
            "wrap" => Command::SetWrap(true),
            "nowrap" => Command::SetWrap(false),
            _ => Command::Unknown(trimmed.to_string()),
        },
        "wrap" => Command::SetWrap(true),
        "nowrap" => Command::SetWrap(false),
        _ => Command::Unknown(trimmed.to_string()),
    }
}

/// Parse a bare-address command — vim's jump-to-line — or `None` when
/// `trimmed` is not one. `:42` and `:$` jump to that line; a range (`:1,5`)
/// jumps to its end, and `:%` (the whole-buffer range) to the last line, both
/// mirroring vim. Anything after the address means some other command (`:5s/…`
/// never reaches here — substitution parses first).
fn parse_goto(trimmed: &str) -> Option<Command> {
    if trimmed == "%" {
        return Some(Command::Goto(Addr::Last));
    }
    let (first, rest) = split_addr(trimmed)?;
    let (addr, rest) = match rest.strip_prefix(',') {
        Some(after_comma) => split_addr(after_comma)?,
        None => (first, rest),
    };
    rest.is_empty().then_some(Command::Goto(addr))
}

/// Split one leading line address (`12`, `.`, `$`) off `s`, returning it and
/// the remainder, or `None` when `s` starts with none.
fn split_addr(s: &str) -> Option<(Addr, &str)> {
    if let Some(rest) = s.strip_prefix('.') {
        return Some((Addr::Current, rest));
    }
    if let Some(rest) = s.strip_prefix('$') {
        return Some((Addr::Last, rest));
    }
    let digits = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digits == 0 {
        return None;
    }
    let n: usize = s[..digits].parse().ok()?;
    Some((Addr::Line(n), &s[digits..]))
}

/// Parse a `:s` / `:%s` / `:N,Ms` substitution, or `None` if `trimmed` is not
/// one (so the caller falls through to the regular verb table). An optional
/// range prefix — `%`, or one/two addresses (`5`, `1,5`, `.,$`) — precedes the
/// `s`. The character right after `s` is the delimiter (vim style), which must
/// be punctuation so real commands like `sort` and a bare `s` are not mistaken
/// for substitutions. The trailing delimiter before the flags is optional.
/// Flags are `g` (all matches on a line), `i` (ignore case), and `I` (force
/// case — the default; wins over `i`); unknown flags make the whole thing
/// [`Command::Unknown`] so the user gets a clear error.
fn parse_substitute(trimmed: &str) -> Option<Command> {
    let (whole_buffer, range, rest) = if let Some(rest) = trimmed.strip_prefix('%') {
        (true, None, rest)
    } else if let Some((from, after)) = split_addr(trimmed) {
        match after.strip_prefix(',') {
            Some(after_comma) => {
                let (to, rest) = split_addr(after_comma)?;
                (false, Some((from, to)), rest)
            }
            None => (false, Some((from, from)), after),
        }
    } else {
        (false, None, trimmed)
    };
    let after_s = rest.strip_prefix('s')?;
    let mut chars = after_s.chars();
    let delim = chars.next()?;
    if delim.is_alphanumeric() || delim.is_whitespace() {
        return None;
    }
    let mut fields = chars.as_str().splitn(3, delim);
    let pattern = fields.next().unwrap_or("").to_string();
    let replacement = fields.next().unwrap_or("").to_string();
    let flags = fields.next().unwrap_or("");
    if !flags.chars().all(|c| matches!(c, 'g' | 'i' | 'I')) {
        return Some(Command::Unknown(trimmed.to_string()));
    }
    Some(Command::Substitute(Substitution {
        whole_buffer,
        range,
        pattern,
        replacement,
        global: flags.contains('g'),
        ignore_case: flags.contains('i') && !flags.contains('I'),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_write_quit_variants() {
        assert_eq!(parse("w"), Command::Write);
        assert_eq!(parse("write"), Command::Write);
        assert_eq!(parse("q"), Command::Quit);
        assert_eq!(parse("wq"), Command::WriteQuit);
        assert_eq!(parse("x"), Command::WriteQuit);
    }

    #[test]
    fn write_with_a_path_is_save_as() {
        assert_eq!(parse("w"), Command::Write);
        assert_eq!(
            parse("w notes.ptl"),
            Command::WriteAs("notes.ptl".to_string())
        );
        assert_eq!(
            parse("write  /tmp/out.ptl"),
            Command::WriteAs("/tmp/out.ptl".to_string())
        );
    }

    #[test]
    fn parses_save_all_variants() {
        assert_eq!(parse("wa"), Command::WriteAll);
        assert_eq!(parse("wall"), Command::WriteAll);
        assert_eq!(parse("wqa"), Command::WriteAllQuit);
        assert_eq!(parse("xa"), Command::WriteAllQuit);
    }

    #[test]
    fn parses_set_wrap() {
        assert_eq!(parse("set wrap"), Command::SetWrap(true));
        assert_eq!(parse("set nowrap"), Command::SetWrap(false));
        assert_eq!(parse("wrap"), Command::SetWrap(true));
        assert_eq!(parse("nowrap"), Command::SetWrap(false));
        assert_eq!(
            parse("set bogus"),
            Command::Unknown("set bogus".to_string())
        );
    }

    #[test]
    fn parses_edit_with_path() {
        assert_eq!(
            parse("e src/main.rs"),
            Command::Edit("src/main.rs".to_string())
        );
        assert_eq!(
            parse("edit  /tmp/foo.txt"),
            Command::Edit("/tmp/foo.txt".to_string())
        );
    }

    #[test]
    fn edit_without_path_yields_empty_arg() {
        assert_eq!(parse("e"), Command::Edit(String::new()));
    }

    #[test]
    fn parses_explore_variants() {
        assert_eq!(parse("E"), Command::Explore);
        assert_eq!(parse("Explore"), Command::Explore);
        assert_eq!(parse("Ex"), Command::Explore);
    }

    #[test]
    fn parses_git() {
        assert_eq!(parse("Git"), Command::Git);
    }

    #[test]
    fn parses_goto_line() {
        assert_eq!(parse("42"), Command::Goto(Addr::Line(42)));
        assert_eq!(parse("$"), Command::Goto(Addr::Last));
        assert_eq!(parse("%"), Command::Goto(Addr::Last));
        assert_eq!(parse("."), Command::Goto(Addr::Current));
        // A range with no command jumps to its end line, like vim.
        assert_eq!(parse("1,5"), Command::Goto(Addr::Line(5)));
        assert_eq!(parse(".,$"), Command::Goto(Addr::Last));
    }

    #[test]
    fn goto_does_not_swallow_addressed_commands() {
        // An address followed by a substitution stays a substitution…
        assert!(matches!(parse("5s/a/b/"), Command::Substitute(_)));
        // …and an address followed by anything else is not a goto.
        assert_eq!(parse("5x"), Command::Unknown("5x".to_string()));
    }

    #[test]
    fn parses_state_toggle() {
        assert_eq!(parse("State"), Command::ToggleState);
    }

    #[test]
    fn parses_history_navigation() {
        assert_eq!(parse("back"), Command::Back);
        assert_eq!(parse("forward"), Command::Forward);
        assert_eq!(parse("fwd"), Command::Forward);
    }

    #[test]
    fn parses_review_and_pr() {
        assert_eq!(parse("Review"), Command::Review(String::new()));
        assert_eq!(parse("Review main"), Command::Review("main".to_string()));
        assert_eq!(
            parse("Pr origin/main"),
            Command::Review("origin/main".to_string())
        );
        assert_eq!(parse("PR"), Command::Pr(String::new()));
        assert_eq!(parse("PR 42"), Command::Pr("42".to_string()));
        // `:Revert` retired with the in-host projection review — an unknown command now.
        assert!(matches!(parse("Revert"), Command::Unknown(_)));
        // The split before/after review and its aliases.
        assert_eq!(parse("Review2"), Command::ReviewSplit(String::new()));
        assert_eq!(
            parse("ReviewSplit main"),
            Command::ReviewSplit("main".to_string())
        );
        assert_eq!(
            parse("Review2 origin/main"),
            Command::ReviewSplit("origin/main".to_string())
        );
    }

    #[test]
    fn parses_diff() {
        assert_eq!(
            parse("Diff"),
            Command::Diff {
                rev: String::new(),
                stat: false
            }
        );
        assert_eq!(
            parse("Diff 55c588c"),
            Command::Diff {
                rev: "55c588c".to_string(),
                stat: false
            }
        );
        assert_eq!(
            parse("D HEAD~2..HEAD"),
            Command::Diff {
                rev: "HEAD~2..HEAD".to_string(),
                stat: false
            }
        );
        // `--stat` selects the summary diagram, optionally with a revision.
        assert_eq!(
            parse("Diff --stat"),
            Command::Diff {
                rev: String::new(),
                stat: true
            }
        );
        assert_eq!(
            parse("Diff --stat HEAD~2..HEAD"),
            Command::Diff {
                rev: "HEAD~2..HEAD".to_string(),
                stat: true
            }
        );
    }

    #[test]
    fn parses_report_with_and_without_text() {
        assert_eq!(
            parse("report dd deletes the wrong line"),
            Command::Report("dd deletes the wrong line".to_string()),
        );
        // A bare `:report` carries an empty description.
        assert_eq!(parse("report"), Command::Report(String::new()));
    }

    #[test]
    fn empty_and_unknown() {
        assert_eq!(parse("   "), Command::Empty);
        assert_eq!(
            parse("frobnicate"),
            Command::Unknown("frobnicate".to_string())
        );
    }

    fn sub(whole_buffer: bool, pattern: &str, replacement: &str, global: bool) -> Command {
        Command::Substitute(Substitution {
            whole_buffer,
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            global,
            range: None,
            ignore_case: false,
        })
    }

    /// A ranged substitution with default flags, for the range-parsing tests.
    fn ranged_sub(range: (Addr, Addr), pattern: &str, replacement: &str) -> Command {
        Command::Substitute(Substitution {
            whole_buffer: false,
            range: Some(range),
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            global: false,
            ignore_case: false,
        })
    }

    #[test]
    fn parses_substitute_line_ranges() {
        assert_eq!(
            parse("1,5s/a/b/"),
            ranged_sub((Addr::Line(1), Addr::Line(5)), "a", "b")
        );
        assert_eq!(
            parse("5s/a/b/"),
            ranged_sub((Addr::Line(5), Addr::Line(5)), "a", "b")
        );
        assert_eq!(
            parse(".,$s/a/b/"),
            ranged_sub((Addr::Current, Addr::Last), "a", "b")
        );
        assert_eq!(
            parse("$s/a/b/"),
            ranged_sub((Addr::Last, Addr::Last), "a", "b")
        );
    }

    #[test]
    fn substitute_range_needs_a_complete_address_pair() {
        // A dangling comma is not a substitution; it falls through to Unknown.
        assert_eq!(parse("1,s/a/b/"), Command::Unknown("1,s/a/b/".to_string()));
    }

    #[test]
    fn parses_substitute_case_flags() {
        let Command::Substitute(s) = parse("s/a/b/i") else {
            panic!("not a substitution")
        };
        assert!(s.ignore_case && !s.global);
        let Command::Substitute(s) = parse("s/a/b/gi") else {
            panic!("not a substitution")
        };
        assert!(s.ignore_case && s.global);
        // `I` forces the case-sensitive default, beating `i`.
        let Command::Substitute(s) = parse("s/a/b/iI") else {
            panic!("not a substitution")
        };
        assert!(!s.ignore_case);
    }

    #[test]
    fn parses_substitute_current_line() {
        assert_eq!(parse("s/foo/bar/"), sub(false, "foo", "bar", false));
        assert_eq!(parse("s/foo/bar/g"), sub(false, "foo", "bar", true));
    }

    #[test]
    fn parses_substitute_whole_buffer() {
        assert_eq!(parse("%s/foo/bar/"), sub(true, "foo", "bar", false));
        assert_eq!(parse("%s/foo/bar/g"), sub(true, "foo", "bar", true));
    }

    #[test]
    fn substitute_trailing_delimiter_is_optional() {
        assert_eq!(parse("s/foo/bar"), sub(false, "foo", "bar", false));
    }

    #[test]
    fn substitute_replacement_can_be_empty() {
        assert_eq!(parse("s/foo//"), sub(false, "foo", "", false));
        assert_eq!(parse("s/foo//g"), sub(false, "foo", "", true));
    }

    #[test]
    fn substitute_empty_pattern_is_preserved_for_the_app() {
        // An empty pattern means "reuse the last search"; the App decides.
        assert_eq!(parse("s//bar/g"), sub(false, "", "bar", true));
    }

    #[test]
    fn substitute_accepts_an_arbitrary_delimiter() {
        assert_eq!(parse("s#a/b#c/d#g"), sub(false, "a/b", "c/d", true));
        assert_eq!(parse("%s,x,y,"), sub(true, "x", "y", false));
    }

    #[test]
    fn substitute_rejects_unknown_flags() {
        assert_eq!(parse("s/a/b/x"), Command::Unknown("s/a/b/x".to_string()));
    }

    #[test]
    fn non_substitute_s_words_are_not_substitutions() {
        // A delimiter must be punctuation, so real words starting with 's'
        // (or a bare ":s") never parse as a substitution.
        assert_eq!(parse("sort"), Command::Unknown("sort".to_string()));
        assert_eq!(parse("s"), Command::Unknown("s".to_string()));
    }

    #[test]
    fn parses_nohlsearch_variants() {
        assert_eq!(parse("noh"), Command::NoHighlight);
        assert_eq!(parse("nohl"), Command::NoHighlight);
        assert_eq!(parse("nohlsearch"), Command::NoHighlight);
    }

    #[test]
    fn search_prompts_display_their_prefix() {
        let mut cl = CommandLine::new_search(true);
        cl.push('f');
        cl.push('o');
        assert_eq!(cl.kind, CommandLineKind::SearchForward);
        assert_eq!(cl.display(), "/fo");
        assert_eq!(CommandLine::new_search(false).display(), "?");
        assert_eq!(CommandLine::new().display(), ":");
    }

    #[test]
    fn backspace_reports_when_empty() {
        let mut cl = CommandLine::new();
        cl.push('a');
        cl.push('b');
        assert!(cl.backspace()); // removed 'b'
        assert!(cl.backspace()); // removed 'a'
        assert!(!cl.backspace()); // now empty
    }
}
