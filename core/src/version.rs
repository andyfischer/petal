//! What this build of Petal is: the crate version, the git commit it was
//! compiled from and the date it was compiled, recorded by `core/build.rs`.
//!
//! `petal --version` prints [`describe`]. It exists because a stale binary is
//! otherwise indistinguishable from a current one: two reviewers of one
//! project were misled by a three-week-old `petal` on their `PATH` that
//! rejected syntax the tree in front of them accepted. An embedder can print
//! the same line for the Petal it vendors.

/// The crate version (`0.1.0`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit the build's sources were at: a ten-digit abbreviated hash,
/// with `-dirty` appended when tracked files had uncommitted changes, or
/// `-vendored` when the sources are a copy inside another repository whose
/// `VENDORED.md` names the commit. `unknown` when neither git nor a
/// `VENDORED.md` could say.
pub const GIT_COMMIT: &str = env!("PETAL_GIT_COMMIT");

/// The date of that commit (`YYYY-MM-DD`), or empty when it is not known.
pub const GIT_DATE: &str = env!("PETAL_GIT_DATE");

/// The UTC date (`YYYY-MM-DD`) the sources were last compiled from a changed
/// state. `SOURCE_DATE_EPOCH` in the build's environment pins it.
pub const BUILD_DATE: &str = env!("PETAL_BUILD_DATE");

/// One line saying all of it: `petal 0.1.0 (4d1b86b0c1 2026-10-13, built
/// 2026-10-14)`.
pub fn describe() -> String {
    let commit = if GIT_DATE.is_empty() {
        GIT_COMMIT.to_string()
    } else {
        format!("{GIT_COMMIT} {GIT_DATE}")
    };
    format!("petal {VERSION} ({commit}, built {BUILD_DATE})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_date(s: &str) -> bool {
        let b = s.as_bytes();
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    }

    #[test]
    fn the_build_records_a_commit_and_a_date() {
        assert!(is_date(BUILD_DATE), "{BUILD_DATE}");
        assert!(GIT_DATE.is_empty() || is_date(GIT_DATE), "{GIT_DATE}");
        assert!(!GIT_COMMIT.is_empty());
        // A hash (with its marker), or the word for not knowing.
        let hash = GIT_COMMIT
            .trim_end_matches("-dirty")
            .trim_end_matches("-vendored");
        assert!(
            GIT_COMMIT == "unknown" || (hash.len() >= 7 && hash.bytes().all(|b| b.is_ascii_hexdigit())),
            "{GIT_COMMIT}"
        );
    }

    #[test]
    fn describe_is_one_line_with_all_three() {
        let line = describe();
        assert!(line.starts_with(&format!("petal {VERSION} (")), "{line}");
        assert!(line.contains(GIT_COMMIT) && line.contains(BUILD_DATE), "{line}");
        assert!(line.ends_with(')') && !line.contains('\n'), "{line}");
    }
}
