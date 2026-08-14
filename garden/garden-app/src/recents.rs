//! What the user opened recently: files, projects (git repo roots), and PRs.
//!
//! Backed by the v3 tables of the shared state database (see [`crate::state`]).
//! Startup consumes the [`State`](crate::state::State) into the
//! [`EventLog`](crate::event_log::EventLog), so [`Recents`] holds its *own*
//! connection to the same `db.sqlite`; WAL (set by
//! [`open_db`](crate::state::open_db)) makes the two writers safe.
//!
//! Every recorded path is absolute and canonicalized, so the same file reached
//! through a relative path, a symlinked checkout, or a different working
//! directory upserts one row rather than sprouting near-duplicates.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::Serialize;

/// A file the user opened, newest-first in [`Recents::recent_files`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecentFile {
    pub path: String,
    /// Repo root the file lives under, absent for files outside any repo.
    pub project_path: Option<String>,
    pub last_opened_ms: i64,
    pub open_count: i64,
}

/// A project (git repo root) the user opened a file from, or opened directly.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecentProject {
    pub path: String,
    /// Final path component of `path` — what the UI shows as the project name.
    pub name: String,
    pub last_opened_ms: i64,
    pub open_count: i64,
}

/// A GitHub pull request the user reviewed, keyed by `(repo, number)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecentPr {
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub project_path: Option<String>,
    pub last_opened_ms: i64,
}

/// The nearest ancestor of `path` (starting at `path` itself when it is a
/// directory) that contains a `.git` entry, or `None` outside any repo.
///
/// Matches a `.git` **file** as well as a directory: worktrees and submodules
/// record a gitdir pointer in a plain file, and treating those as
/// "not a repo" would drop exactly the checkouts most likely to be transient.
pub fn project_root(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Absolute, symlink-resolved form of `path`, falling back to `path` itself
/// when it cannot be resolved — recording a file the user is *about to*
/// create is more useful than recording nothing.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Insert-or-touch a project row on any connection-like handle, so
/// [`Recents::record_file`] can run it inside its transaction.
fn upsert_project(conn: &Connection, path: &Path, at_ms: i64) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    conn.execute(
        "INSERT INTO recent_projects (path, name, last_opened_ms, open_count)
         VALUES (?1, ?2, ?3, 1)
         ON CONFLICT(path) DO UPDATE SET
             name = excluded.name,
             last_opened_ms = excluded.last_opened_ms,
             open_count = open_count + 1",
        rusqlite::params![path.to_string_lossy(), name, at_ms],
    )
    .map_err(|e| format!("record recent project: {e}"))?;
    Ok(())
}

/// Reader/writer for the recents tables. Cheap to open; holds one connection.
pub struct Recents {
    conn: Connection,
}

impl Recents {
    /// Open (and migrate) the state database under `state_dir` on a fresh
    /// connection of its own.
    pub fn open(state_dir: &Path) -> Result<Recents, String> {
        Ok(Recents {
            conn: crate::state::open_db(state_dir)?,
        })
    }

    /// Record a file open, and — in the same transaction — the project it
    /// belongs to, so the two lists can never disagree about a given open.
    pub fn record_file(&self, path: &Path) -> Result<(), String> {
        let path = canonical(path);
        let project = project_root(&path);
        let at_ms = now_ms();

        self.conn
            .execute_batch("BEGIN")
            .map_err(|e| format!("begin recents write: {e}"))?;
        let result = (|| {
            if let Some(project) = &project {
                upsert_project(&self.conn, project, at_ms)?;
            }
            self.conn
                .execute(
                    "INSERT INTO recent_files (path, project_path, last_opened_ms, open_count)
                     VALUES (?1, ?2, ?3, 1)
                     ON CONFLICT(path) DO UPDATE SET
                         project_path = excluded.project_path,
                         last_opened_ms = excluded.last_opened_ms,
                         open_count = open_count + 1",
                    rusqlite::params![
                        path.to_string_lossy(),
                        project.as_ref().map(|p| p.to_string_lossy().into_owned()),
                        at_ms
                    ],
                )
                .map_err(|e| format!("record recent file: {e}"))?;
            Ok(())
        })();
        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT")
                .map_err(|e| format!("commit recents write: {e}")),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Record a project open directly (no file involved).
    pub fn record_project(&self, path: &Path) -> Result<(), String> {
        upsert_project(&self.conn, &canonical(path), now_ms())
    }

    /// Record a PR review. Unlike files and projects a PR has no `open_count`:
    /// its list is short and always ranked purely by recency.
    pub fn record_pr(
        &self,
        repo: &str,
        number: i64,
        title: &str,
        project: Option<&Path>,
    ) -> Result<(), String> {
        let project = project.map(|p| canonical(p).to_string_lossy().into_owned());
        self.conn
            .execute(
                "INSERT INTO recent_prs (repo, number, title, project_path, last_opened_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(repo, number) DO UPDATE SET
                     title = excluded.title,
                     project_path = excluded.project_path,
                     last_opened_ms = excluded.last_opened_ms",
                rusqlite::params![repo, number, title, project, now_ms()],
            )
            .map_err(|e| format!("record recent pr: {e}"))?;
        Ok(())
    }

    pub fn recent_files(&self, limit: usize) -> Result<Vec<RecentFile>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, project_path, last_opened_ms, open_count FROM recent_files
                 ORDER BY last_opened_ms DESC LIMIT ?1",
            )
            .map_err(|e| format!("query recent files: {e}"))?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(RecentFile {
                    path: row.get(0)?,
                    project_path: row.get(1)?,
                    last_opened_ms: row.get(2)?,
                    open_count: row.get(3)?,
                })
            })
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|e| format!("read recent files: {e}"))?;
        Ok(rows)
    }

    pub fn recent_projects(&self, limit: usize) -> Result<Vec<RecentProject>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, name, last_opened_ms, open_count FROM recent_projects
                 ORDER BY last_opened_ms DESC LIMIT ?1",
            )
            .map_err(|e| format!("query recent projects: {e}"))?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(RecentProject {
                    path: row.get(0)?,
                    name: row.get(1)?,
                    last_opened_ms: row.get(2)?,
                    open_count: row.get(3)?,
                })
            })
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|e| format!("read recent projects: {e}"))?;
        Ok(rows)
    }

    pub fn recent_prs(&self, limit: usize) -> Result<Vec<RecentPr>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT repo, number, title, project_path, last_opened_ms FROM recent_prs
                 ORDER BY last_opened_ms DESC LIMIT ?1",
            )
            .map_err(|e| format!("query recent prs: {e}"))?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(RecentPr {
                    repo: row.get(0)?,
                    number: row.get(1)?,
                    title: row.get(2)?,
                    project_path: row.get(3)?,
                    last_opened_ms: row.get(4)?,
                })
            })
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|e| format!("read recent prs: {e}"))?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A file's recorded timestamp comes from the wall clock in milliseconds,
    /// so two writes in the same millisecond would tie and make ordering
    /// assertions flaky. Force the recorded order instead of sleeping.
    fn set_opened_at(recents: &Recents, path: &str, at_ms: i64) {
        recents
            .conn
            .execute(
                "UPDATE recent_files SET last_opened_ms = ?2 WHERE path = ?1",
                rusqlite::params![path, at_ms],
            )
            .unwrap();
    }

    fn canon(path: &Path) -> String {
        canonical(path).to_string_lossy().into_owned()
    }

    #[test]
    fn project_root_finds_git_directory() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        let file = repo.join("src/main.rs");
        fs::write(&file, "").unwrap();
        assert_eq!(project_root(&file), Some(repo));
    }

    /// Worktrees and submodules point at their real gitdir with a `.git`
    /// *file*, which must count as a repo root just like the directory form.
    #[test]
    fn project_root_finds_git_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("worktree");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(".git"), "gitdir: /elsewhere/.git/worktrees/wt\n").unwrap();
        let file = repo.join("a.txt");
        fs::write(&file, "").unwrap();
        assert_eq!(project_root(&file), Some(repo));
    }

    #[test]
    fn project_root_is_none_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("loose.txt");
        fs::write(&file, "").unwrap();
        assert_eq!(project_root(&file), None);
    }

    /// A repo checked out inside another repo (submodule, vendored clone):
    /// the innermost root wins, since that is the project being edited.
    #[test]
    fn project_root_picks_the_nearest_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("outer");
        let inner = outer.join("vendor/inner");
        fs::create_dir_all(outer.join(".git")).unwrap();
        fs::create_dir_all(inner.join(".git")).unwrap();
        fs::create_dir_all(inner.join("src")).unwrap();
        assert_eq!(project_root(&inner.join("src")), Some(inner));
    }

    #[test]
    fn open_on_a_fresh_dir_migrates_and_works() {
        let dir = tempfile::tempdir().unwrap();
        let recents = Recents::open(dir.path()).unwrap();
        assert!(dir.path().join("db.sqlite").exists());
        assert!(recents.recent_files(10).unwrap().is_empty());
        assert!(recents.recent_projects(10).unwrap().is_empty());
        assert!(recents.recent_prs(10).unwrap().is_empty());
    }

    #[test]
    fn record_file_inserts_then_upserts_and_records_the_project() {
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let repo = work.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let file = repo.join("a.txt");
        fs::write(&file, "").unwrap();

        let recents = Recents::open(state.path()).unwrap();
        recents.record_file(&file).unwrap();
        recents.record_file(&file).unwrap();

        let files = recents.recent_files(10).unwrap();
        assert_eq!(files.len(), 1, "re-opening a file must not duplicate it");
        assert_eq!(files[0].path, canon(&file));
        assert_eq!(files[0].open_count, 2);
        assert_eq!(files[0].project_path, Some(canon(&repo)));

        let projects = recents.recent_projects(10).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path, canon(&repo));
        assert_eq!(projects[0].name, "repo");
        assert_eq!(projects[0].open_count, 2);
    }

    #[test]
    fn files_outside_a_repo_record_no_project() {
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let file = work.path().join("loose.txt");
        fs::write(&file, "").unwrap();

        let recents = Recents::open(state.path()).unwrap();
        recents.record_file(&file).unwrap();

        assert_eq!(recents.recent_files(10).unwrap()[0].project_path, None);
        assert!(recents.recent_projects(10).unwrap().is_empty());
    }

    /// A path the user is about to create cannot be canonicalized; it is still
    /// recorded, absolutized rather than dropped.
    #[test]
    fn record_file_accepts_a_path_that_does_not_exist_yet() {
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let file = work.path().join("new.txt");

        let recents = Recents::open(state.path()).unwrap();
        recents.record_file(&file).unwrap();

        let files = recents.recent_files(10).unwrap();
        assert_eq!(files.len(), 1);
        assert!(Path::new(&files[0].path).is_absolute());
        assert!(files[0].path.ends_with("new.txt"));
    }

    #[test]
    fn lists_are_most_recent_first_and_respect_the_limit() {
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let recents = Recents::open(state.path()).unwrap();

        let mut paths = Vec::new();
        for (i, name) in ["a.txt", "b.txt", "c.txt"].iter().enumerate() {
            let file = work.path().join(name);
            fs::write(&file, "").unwrap();
            recents.record_file(&file).unwrap();
            let path = canon(&file);
            set_opened_at(&recents, &path, 1000 + i as i64);
            paths.push(path);
        }

        let files = recents.recent_files(10).unwrap();
        let ordered: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(ordered, vec![&paths[2], &paths[1], &paths[0]]);

        let limited = recents.recent_files(2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].path, paths[2]);
    }

    #[test]
    fn record_pr_upserts_on_repo_and_number() {
        let state = tempfile::tempdir().unwrap();
        let recents = Recents::open(state.path()).unwrap();

        recents
            .record_pr("owner/repo", 12, "First title", None)
            .unwrap();
        recents
            .record_pr("owner/repo", 12, "Renamed title", None)
            .unwrap();
        // Same number in a different repo is a different PR.
        recents
            .record_pr("other/repo", 12, "Elsewhere", None)
            .unwrap();

        let prs = recents.recent_prs(10).unwrap();
        assert_eq!(prs.len(), 2);
        let mine = prs.iter().find(|p| p.repo == "owner/repo").unwrap();
        assert_eq!(mine.title, "Renamed title");
        assert_eq!(mine.number, 12);
    }

    /// [`Recents`] opens its own connection because startup has already moved
    /// the `State` connection into the event log — so it must work against a
    /// database another handle is holding open at the same time.
    #[test]
    fn works_alongside_an_open_state_connection() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::State::open(dir.path()).unwrap();
        let recents = Recents::open(dir.path()).unwrap();

        state.new_window_id().unwrap();
        recents
            .record_pr("owner/repo", 1, "Concurrent", None)
            .unwrap();
        assert_eq!(recents.recent_prs(10).unwrap().len(), 1);
    }
}
