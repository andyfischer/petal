// The package manager: git dependencies, petal.lock, vendor/, bundles
// (src/pkgman.rs, src/cli/pm.rs, docs/module-system.md#the-package-manager).
//
// Nothing here touches the network: every "remote" is a git repository built
// in a temp directory and named by path or file:// url, and every command
// runs with PETAL_HOME pointed at a temp directory of its own.

use std::path::{Path, PathBuf};
use std::process::Command;

use petal::env::Env;
use petal::pkgman::{Lock, LockedSource};

/// A scratch directory holding the remotes, the project and the petal home
/// of one test.
struct World {
    root: PathBuf,
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl World {
    fn new(tag: &str) -> World {
        let root = std::env::temp_dir().join(format!("petal-pkgman-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        World {
            root: std::fs::canonicalize(&root).unwrap(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).unwrap()
    }

    fn git(&self, repo: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=petal",
                "-c",
                "user.email=petal@example.invalid",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .current_dir(self.path(repo))
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} in {repo}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Commit everything in `repo` (initializing it first) and tag it.
    fn commit(&self, repo: &str, tag: &str) -> String {
        if !self.path(repo).join(".git").exists() {
            self.git(repo, &["init", "--quiet"]);
        }
        self.git(repo, &["add", "-A"]);
        self.git(repo, &["commit", "--quiet", "-m", tag]);
        self.git(repo, &["tag", "--force", tag]);
        self.git(repo, &["rev-parse", "HEAD"])
    }

    fn petal_env(&self, dir: &str, args: &[&str], env: &[(&str, &str)]) -> Out {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_petal"));
        cmd.args(args)
            .current_dir(self.path(dir))
            .env("PETAL_HOME", self.path("home"))
            .env_remove("PETAL_PATH")
            .env_remove("PETAL_OFFLINE");
        for (key, value) in env {
            cmd.env(key, value);
        }
        let out = cmd.output().expect("petal runs");
        Out {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn petal(&self, dir: &str, args: &[&str]) -> Out {
        self.petal_env(dir, args, &[])
    }

    /// Run a command that must succeed; its stdout.
    fn ok(&self, dir: &str, args: &[&str]) -> String {
        let out = self.petal(dir, args);
        assert!(out.ok, "petal {args:?} failed: {}\n{}", out.stderr, out.stdout);
        out.stdout
    }

    /// Run a command that must fail; its stderr.
    fn fails(&self, dir: &str, args: &[&str]) -> String {
        let out = self.petal(dir, args);
        assert!(!out.ok, "petal {args:?} should have failed: {}", out.stdout);
        out.stderr
    }

    fn lock(&self, dir: &str) -> Lock {
        Lock::load(&self.path(dir)).unwrap().expect("a lockfile")
    }

    /// The `ease` library: v1.0.0 has `quad`, v1.1.0 adds `cubic`.
    fn ease_repo(&self) -> (String, String) {
        self.write("remotes/ease/petal.toml", "[package]\nname = \"ease\"\nversion = \"1.0.0\"\n");
        self.write("remotes/ease/src/ease.ptl", "pub fn quad(t)\n  t * t\nend\n");
        let one = self.commit("remotes/ease", "v1.0.0");
        self.write("remotes/ease/petal.toml", "[package]\nname = \"ease\"\nversion = \"1.1.0\"\n");
        self.write(
            "remotes/ease/src/ease.ptl",
            "pub fn quad(t)\n  t * t\nend\npub fn cubic(t)\n  t * t * t\nend\n",
        );
        let two = self.commit("remotes/ease", "v1.1.0");
        (one, two)
    }

    /// The `tween` library: a git dependency on ease v1.1.0 and a path
    /// dependency on a package inside its own repository.
    fn tween_repo(&self) {
        self.write(
            "remotes/tween/petal.toml",
            &format!(
                "[package]\nname = \"tween\"\nversion = \"0.3.0\"\n\n[dependencies]\n\
                 ease = {{ git = \"{}\", tag = \"v1.1.0\" }}\n\
                 shapes = {{ path = \"libs/shapes\" }}\n\n[requires]\ncore = \">=0.1\"\n",
                self.path("remotes/ease").display()
            ),
        );
        self.write(
            "remotes/tween/src/tween.ptl",
            "import ease\nimport shapes\npub fn go(t)\n  ease.cubic(t) + sides()\nend\n",
        );
        self.write("remotes/tween/libs/shapes/petal.toml", "[package]\nname = \"shapes\"\n");
        self.write("remotes/tween/libs/shapes/shapes.ptl", "pub fn sides()\n  4\nend\n");
        self.commit("remotes/tween", "v0.3.0");
    }

    /// The project: depends on tween (which wants ease v1.1.0) and itself
    /// asks for ease v1.0.0.
    fn app(&self) {
        self.write(
            "app/petal.toml",
            &format!(
                "[package]\nname = \"game\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
                 tween = {{ git = \"file://{}\", tag = \"v0.3.0\" }}\n\
                 ease = {{ git = \"{}\", tag = \"v1.0.0\" }}\n",
                self.path("remotes/tween").display(),
                self.path("remotes/ease").display()
            ),
        );
        self.write(
            "app/main.ptl",
            "import tween\nimport ease\nimport ./level\n\
             print(tween.go(2), quad(3), cubic(2), level.name())\n",
        );
        self.write("app/level.ptl", "pub fn name()\n  \"one\"\nend\n");
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git_entry(lock: &Lock, name: &str) -> (Option<String>, String, String) {
    match &lock.package(name).unwrap_or_else(|| panic!("{name} is locked")).source {
        LockedSource::Git {
            tag, commit, tree, ..
        } => (tag.clone(), commit.clone(), tree.clone()),
        other => panic!("{name} is not a git package: {other:?}"),
    }
}

// ── the whole trip ───────────────────────────────────────────────

#[test]
fn install_lock_vendor_bundle_run() {
    let w = World::new("e2e");
    let (_, ease_two) = w.ease_repo();
    w.tween_repo();
    w.app();

    // install: fetch the graph, pick one version of each package, lock it.
    let out = w.ok("app", &["install"]);
    assert!(out.contains("Locked 3 packages in petal.lock"), "{out}");
    let lock = w.lock("app");
    let names: Vec<&str> = lock.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["ease", "shapes", "tween"]);
    // Minimal version selection: the app asks for v1.0.0, tween for v1.1.0.
    let (tag, commit, tree) = git_entry(&lock, "ease");
    assert_eq!(tag.as_deref(), Some("v1.1.0"));
    assert_eq!(commit, ease_two);
    assert!(tree.starts_with("sha256:") && tree.len() == 7 + 64, "{tree}");
    assert_eq!(lock.package("ease").unwrap().version.as_deref(), Some("1.1.0"));
    // The package inside tween's repository is pinned through tween.
    assert_eq!(
        lock.package("shapes").unwrap().source,
        LockedSource::Path {
            path: "libs/shapes".into(),
            within: Some("tween".into())
        }
    );
    assert_eq!(
        lock.package("tween").unwrap().dependencies,
        [("ease".to_string(), "ease".to_string()), ("shapes".to_string(), "shapes".to_string())]
    );
    // The cache is content-addressed.
    let cached = w
        .path("home/cache/trees")
        .join(tree.strip_prefix("sha256:").unwrap());
    assert!(cached.join("src/ease.ptl").is_file());
    assert_eq!(petal::pkgman::hash_tree(&cached).unwrap(), tree);

    // run and check agree, with no flags.
    assert_eq!(w.ok("app", &["run", "main.ptl"]), "12 9 8 one\n");
    let check = w.petal("app", &["check", "main.ptl"]);
    assert!(check.ok, "{}", check.stderr);
    assert!(check.stdout.contains("requires host interfaces: core >=0.1 (tween)"), "{}", check.stdout);

    // A second install changes nothing and fetches nothing.
    let before = w.read("app/petal.lock");
    let again = w.petal_env("app", &["install", "--locked"], &[("PETAL_OFFLINE", "1")]);
    assert!(again.ok, "{}", again.stderr);
    assert!(again.stdout.contains("petal.lock is up to date (3 packages)"), "{}", again.stdout);
    assert_eq!(w.read("app/petal.lock"), before);

    // packages shows the resolved graph.
    let listing = w.ok("app", &["packages"]);
    assert!(listing.contains("tween 0.3.0  (git "), "{listing}");
    assert!(listing.contains(&format!("tag v1.1.0 @ {}", &ease_two[..10])), "{listing}");
    assert!(listing.contains("shapes  (path libs/shapes)"), "{listing}");
    assert!(listing.contains("core >=0.1  (package tween)"), "{listing}");
    let json: serde_json::Value =
        serde_json::from_str(&w.ok("app", &["packages", "--json"])).unwrap();
    assert_eq!(json["project"]["locked"], true);
    let deps = json["project"]["dependencies"].as_array().unwrap();
    assert_eq!(deps[1]["package"], "ease");
    assert_eq!(deps[1]["commit"], ease_two.as_str());
    assert_eq!(deps[1]["vendored"], false);
    assert_eq!(json["errors"].as_array().unwrap().len(), 0, "{json}");

    // vendor: the git packages are copied into the project…
    let out = w.ok("app", &["vendor"]);
    assert!(out.contains("Vendored into vendor/: ease, tween"), "{out}");
    assert!(w.path("app/vendor/tween/libs/shapes/shapes.ptl").is_file());
    assert!(w.path("app/vendor/ease/src/ease.ptl").is_file());
    assert!(!w.path("app/vendor/ease/.git").exists());
    // …and are used in preference to the cache: with the cache gone and
    // fetching forbidden, the program still runs.
    std::fs::remove_dir_all(w.path("home")).unwrap();
    let run = w.petal_env("app", &["run", "main.ptl"], &[("PETAL_OFFLINE", "1")]);
    assert!(run.ok, "{}", run.stderr);
    assert_eq!(run.stdout, "12 9 8 one\n");
    let installed = w.petal_env("app", &["install"], &[("PETAL_OFFLINE", "1")]);
    assert!(installed.ok, "{}", installed.stderr);
    assert!(installed.stdout.contains("(vendored)"), "{}", installed.stdout);
    assert!(!w.path("home").exists(), "a vendored project needs no cache");

    // bundle: every module of the graph, in the form register_package takes.
    let bundle: serde_json::Value =
        serde_json::from_str(&w.ok("app", &["bundle", "main.ptl"])).unwrap();
    assert_eq!(bundle["format"], 1);
    let packages = bundle["packages"].as_array().unwrap();
    let names: Vec<&str> = packages.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["game", "ease", "shapes", "tween"]);
    assert_eq!(bundle["requires"][0]["interface"], "core");
    assert_eq!(bundle["requires"][0]["package"], "tween");
    assert_eq!(bundle["entry"]["file"], "main.ptl");
    assert_eq!(bundle["entry"]["path"], "<package:game>/main.ptl");

    // A host with no filesystem registers the packages and runs the entry.
    let mut env = Env::new();
    for package in packages {
        let modules: Vec<(String, String)> = package["modules"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, source)| (name.clone(), source.as_str().unwrap().to_string()))
            .collect();
        env.register_package(package["name"].as_str().unwrap(), modules)
            .unwrap();
    }
    let source = bundle["entry"]["source"].as_str().unwrap();
    let path = Path::new(bundle["entry"]["path"].as_str().unwrap());
    let pid = env.load_program_at(source, path).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["12 9 8 one"]);

    // -o writes the same document to a file.
    w.ok("app", &["bundle", "main.ptl", "-o", "out.json"]);
    let written: serde_json::Value = serde_json::from_str(&w.read("app/out.json")).unwrap();
    assert_eq!(written, bundle);
}

// ── install on demand ────────────────────────────────────────────

#[test]
fn check_and_run_install_what_a_program_imports() {
    let w = World::new("ondemand");
    w.ease_repo();
    w.tween_repo();
    w.app();
    // A script that imports no git dependency fetches nothing.
    w.write("app/plain.ptl", "import ./level\nprint(level.name())\n");
    assert_eq!(w.ok("app", &["run", "plain.ptl"]), "one\n");
    assert!(!w.path("app/petal.lock").exists());
    assert!(!w.path("home").exists());

    // Offline, the missing install is the error, and it names the cure.
    let out = w.petal_env("app", &["check", "main.ptl"], &[("PETAL_OFFLINE", "1")]);
    assert!(!out.ok);
    assert!(out.stderr.contains("is a git dependency"), "{}", out.stderr);
    assert!(out.stderr.contains("run `petal install`"), "{}", out.stderr);
    assert!(!w.path("app/petal.lock").exists());

    // Otherwise the first import of one installs the project.
    let out = w.petal("app", &["check", "main.ptl"]);
    assert!(out.ok, "{}", out.stderr);
    assert!(out.stderr.contains("Installing the dependencies of"), "{}", out.stderr);
    assert_eq!(w.lock("app").packages.len(), 3);
    // …once: the next command is quiet.
    let out = w.petal("app", &["run", "main.ptl"]);
    assert_eq!(out.stderr, "");
    assert_eq!(out.stdout, "12 9 8 one\n");

    // A manifest that moves past the lockfile is installed again.
    let manifest = w.read("app/petal.toml").replace("v1.0.0", "v1.1.0");
    w.write("app/petal.toml", &manifest);
    assert_eq!(w.petal("app", &["run", "main.ptl"]).stderr, "");
    w.write(
        "remotes/ease/src/ease.ptl",
        "pub fn quad(t)\n  t * t\nend\npub fn cubic(t)\n  1000\nend\n",
    );
    w.commit("remotes/ease", "v1.2.0");
    let manifest = w.read("app/petal.toml").replace("v1.1.0", "v1.2.0");
    w.write("app/petal.toml", &manifest);
    let out = w.petal("app", &["run", "main.ptl"]);
    assert!(out.stderr.contains("Installing the dependencies of"), "{}", out.stderr);
    assert_eq!(out.stdout, "1004 9 1000 one\n");
    assert_eq!(git_entry(&w.lock("app"), "ease").0.as_deref(), Some("v1.2.0"));
}

#[test]
fn an_embedding_host_never_fetches() {
    // Env::new() — what a host and the language server use — has install on
    // demand off: an uninstalled git dependency is an error naming the cure.
    let w = World::new("host");
    w.ease_repo();
    w.write(
        "app/petal.toml",
        &format!(
            "[package]\nname = \"game\"\n[dependencies]\nease = {{ git = \"{}\", tag = \"v1.0.0\" }}\n",
            w.path("remotes/ease").display()
        ),
    );
    let main = w.write("app/main.ptl", "import ease\nprint(quad(2))\n");
    let mut env = Env::new();
    let err = env
        .load_program_at(&std::fs::read_to_string(&main).unwrap(), &main)
        .unwrap_err();
    assert!(err.contains("is not installed"), "{err}");
    assert!(err.contains("run `petal install`"), "{err}");
    assert!(!w.path("app/petal.lock").exists());
}

// ── one version of each package ──────────────────────────────────

#[test]
fn pins_that_cannot_be_ordered_are_a_conflict() {
    let w = World::new("conflict");
    let (ease_one, _) = w.ease_repo();
    w.tween_repo();
    // tween wants ease at tag v1.1.0; the app pins a different commit by rev.
    w.write(
        "app/petal.toml",
        &format!(
            "[package]\nname = \"game\"\n\n[dependencies]\n\
             tween = {{ git = \"{}\", tag = \"v0.3.0\" }}\n\
             ease = {{ git = \"{}\", rev = \"{ease_one}\" }}\n",
            w.path("remotes/tween").display(),
            w.path("remotes/ease").display()
        ),
    );
    let err = w.fails("app", &["install"]);
    assert!(err.contains("is wanted at rev '"), "{err}");
    assert!(err.contains("(by 'game')"), "{err}");
    assert!(err.contains("at tag 'v1.1.0' (by 'tween')"), "{err}");
    assert!(err.contains("exactly one version of each package"), "{err}");
    assert!(!w.path("app/petal.lock").exists());

    // The same commit under two spellings is one version.
    let manifest = w.read("app/petal.toml").replace(&ease_one, &w.git("remotes/ease", &["rev-parse", "v1.1.0"]));
    w.write("app/petal.toml", &manifest);
    w.ok("app", &["install"]);
    assert_eq!(w.lock("app").packages.len(), 3);
}

#[test]
fn two_sources_for_one_package_name_are_a_conflict() {
    let w = World::new("twosources");
    w.ease_repo();
    w.tween_repo();
    // A local directory that also calls itself `ease`.
    w.write("local/ease/petal.toml", "[package]\nname = \"ease\"\n");
    w.write("local/ease/ease.ptl", "pub fn quad(t)\n  0\nend\n");
    w.write(
        "app/petal.toml",
        &format!(
            "[package]\nname = \"game\"\n\n[dependencies]\n\
             tween = {{ git = \"{}\", tag = \"v0.3.0\" }}\n\
             ease = {{ path = \"../local/ease\" }}\n",
            w.path("remotes/tween").display()
        ),
    );
    let err = w.fails("app", &["install"]);
    assert!(err.contains("two different packages are both named 'ease'"), "{err}");
    assert!(err.contains("path ../local/ease"), "{err}");
}

#[test]
fn a_git_packages_path_dependency_may_not_leave_its_repository() {
    let w = World::new("escape");
    w.write("remotes/outside/petal.toml", "[package]\nname = \"outside\"\n");
    w.write(
        "remotes/lib/petal.toml",
        "[package]\nname = \"lib\"\n[dependencies]\noutside = { path = \"../outside\" }\n",
    );
    w.write("remotes/lib/lib.ptl", "pub fn x()\n  1\nend\n");
    w.commit("remotes/lib", "v1.0.0");
    w.write("app/petal.toml", "[package]\nname = \"game\"\n");
    let err = w.fails(
        "app",
        &["add", "lib", "--git", w.path("remotes/lib").to_str().unwrap(), "--tag", "v1.0.0"],
    );
    assert!(err.contains("leaves the repository the package was fetched from"), "{err}");
    assert!(err.contains("petal.toml was left unchanged"), "{err}");
    assert_eq!(w.read("app/petal.toml"), "[package]\nname = \"game\"\n");
}

// ── verification ─────────────────────────────────────────────────

#[test]
fn what_is_fetched_is_verified_against_the_lockfile() {
    let w = World::new("verify");
    w.ease_repo();
    w.write(
        "app/petal.toml",
        &format!(
            "[package]\nname = \"game\"\n[dependencies]\nease = {{ git = \"{}\", tag = \"v1.0.0\" }}\n",
            w.path("remotes/ease").display()
        ),
    );
    w.write("app/main.ptl", "import ease\nprint(quad(4))\n");
    w.ok("app", &["install"]);
    let (_, commit, tree) = git_entry(&w.lock("app"), "ease");
    let cached = w
        .path("home/cache/trees")
        .join(tree.strip_prefix("sha256:").unwrap());

    // A damaged cache entry is noticed by install and fetched again.
    std::fs::write(cached.join("src/ease.ptl"), "pub fn quad(t)\n  666\nend\n").unwrap();
    w.ok("app", &["install"]);
    assert_eq!(w.ok("app", &["run", "main.ptl"]), "16\n");

    // The tag moves upstream: the lockfile still pins the old commit.
    w.write("remotes/ease/src/ease.ptl", "pub fn quad(t)\n  t * t * 100\nend\n");
    let moved = w.commit("remotes/ease", "v1.0.0");
    assert_ne!(moved, commit);
    w.ok("app", &["install"]);
    assert_eq!(git_entry(&w.lock("app"), "ease").1, commit);
    assert_eq!(w.ok("app", &["run", "main.ptl"]), "16\n");
    // A fresh machine (no cache) gets the same thing from the lockfile.
    std::fs::remove_dir_all(w.path("home")).unwrap();
    assert_eq!(w.ok("app", &["run", "main.ptl"]), "16\n");

    // A lockfile whose hash does not match what its commit holds is refused.
    let good = w.read("app/petal.lock");
    w.write("app/petal.lock", &good.replace(&tree, &format!("sha256:{}", "0".repeat(64))));
    std::fs::remove_dir_all(w.path("home")).unwrap();
    let err = w.fails("app", &["install"]);
    assert!(err.contains("does not match petal.lock"), "{err}");
    assert!(err.contains("petal update ease"), "{err}");
    w.write("app/petal.lock", &good);

    // update re-resolves the tag and re-pins.
    let out = w.ok("app", &["update", "ease"]);
    assert!(out.contains("Locked 1 package in petal.lock"), "{out}");
    assert_eq!(git_entry(&w.lock("app"), "ease").1, moved);
    assert_eq!(w.ok("app", &["run", "main.ptl"]), "1600\n");
    let err = w.fails("app", &["update", "nope"]);
    assert!(err.contains("pins no package named 'nope'"), "{err}");

    // Lowering a pin takes effect too: the locked higher version stands
    // only while something in the graph still asks for it.
    w.write("remotes/ease/NOTES", "1.5\n");
    w.commit("remotes/ease", "v1.5.0");
    let manifest = w.read("app/petal.toml").replace("v1.0.0", "v1.5.0");
    w.write("app/petal.toml", &manifest);
    w.ok("app", &["install"]);
    assert_eq!(git_entry(&w.lock("app"), "ease").0.as_deref(), Some("v1.5.0"));
    let manifest = w.read("app/petal.toml").replace("v1.5.0", "v1.0.0");
    w.write("app/petal.toml", &manifest);
    w.ok("app", &["install"]);
    assert_eq!(git_entry(&w.lock("app"), "ease").0.as_deref(), Some("v1.0.0"));

    // --locked refuses to change the lockfile.
    w.write("remotes/ease/src/ease.ptl", "pub fn quad(t)\n  7\nend\n");
    w.commit("remotes/ease", "v2.0.0");
    let manifest = w.read("app/petal.toml").replace("v1.0.0", "v2.0.0");
    w.write("app/petal.toml", &manifest);
    let err = w.fails("app", &["install", "--locked"]);
    assert!(err.contains("--locked forbids changing"), "{err}");
    assert_eq!(git_entry(&w.lock("app"), "ease").0.as_deref(), Some("v1.0.0"));
}

#[test]
fn a_vendored_tree_is_verified_and_kept_in_step() {
    let w = World::new("vendor");
    w.ease_repo();
    w.write(
        "app/petal.toml",
        &format!(
            "[package]\nname = \"game\"\n[dependencies]\nease = {{ git = \"{}\", tag = \"v1.0.0\" }}\n",
            w.path("remotes/ease").display()
        ),
    );
    w.write("app/main.ptl", "import ease\nprint(quad(4))\n");
    w.ok("app", &["vendor"]);
    assert!(w.path("app/vendor/ease/petal.toml").is_file());

    // An edited vendor tree still loads (nothing re-hashes on every run)…
    w.write("app/vendor/ease/src/ease.ptl", "pub fn quad(t)\n  -1\nend\n");
    assert_eq!(w.ok("app", &["run", "main.ptl"]), "-1\n");
    // …but install says so, and vendor puts it right.
    let err = w.fails("app", &["install"]);
    assert!(err.contains("vendor/ease does not match petal.lock"), "{err}");
    w.ok("app", &["vendor"]);
    assert_eq!(w.ok("app", &["run", "main.ptl"]), "16\n");

    // Moving the pin re-vendors: vendor/ follows the lockfile.
    let manifest = w.read("app/petal.toml").replace("v1.0.0", "v1.1.0");
    w.write("app/petal.toml", &manifest);
    let out = w.ok("app", &["install"]);
    assert!(out.contains("Vendored into vendor/: ease"), "{out}");
    assert!(w.read("app/vendor/ease/src/ease.ptl").contains("cubic"));

    // A hand-vendored package answers for a name no manifest lists.
    w.write("app/vendor/extra/petal.toml", "[package]\nname = \"extra\"\n");
    w.write("app/vendor/extra/extra.ptl", "pub fn more()\n  \"more\"\nend\n");
    w.write("app/uses_extra.ptl", "import extra\nprint(more())\n");
    assert_eq!(w.ok("app", &["run", "uses_extra.ptl"]), "more\n");
}

// ── init, add, remove ────────────────────────────────────────────

#[test]
fn init_add_and_remove_edit_the_manifest() {
    let w = World::new("edit");
    w.ease_repo();
    std::fs::create_dir_all(w.path("my-game")).unwrap();
    let out = w.ok("my-game", &["init"]);
    assert!(out.contains("Created "), "{out}");
    assert_eq!(
        w.read("my-game/petal.toml"),
        "[package]\nname = \"my_game\"\nversion = \"0.1.0\"\n"
    );
    let err = w.fails("my-game", &["init"]);
    assert!(err.contains("already exists"), "{err}");
    w.ok("my-game", &["init", "sub", "--name", "inner"]);
    assert!(w.read("my-game/sub/petal.toml").contains("name = \"inner\""));
    std::fs::remove_dir_all(w.path("my-game/sub")).unwrap();

    // A path dependency.
    w.write("libs/util/petal.toml", "[package]\nname = \"util\"\nversion = \"0.2.0\"\n");
    w.write("libs/util/util.ptl", "pub fn twice(x)\n  x * 2\nend\n");
    let out = w.ok("my-game", &["add", "util", "--path", "../libs/util"]);
    assert!(out.contains("Added util to petal.toml"), "{out}");
    assert!(out.contains("util 0.2.0  path ../libs/util"), "{out}");
    // A git dependency, added from a directory below the project root.
    std::fs::create_dir_all(w.path("my-game/screens")).unwrap();
    let ease = w.path("remotes/ease");
    w.ok(
        "my-game/screens",
        &["add", "ease", "--git", ease.to_str().unwrap(), "--tag", "v1.1.0"],
    );
    assert_eq!(
        w.read("my-game/petal.toml"),
        format!(
            "[package]\nname = \"my_game\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
             util = {{ path = \"../libs/util\" }}\n\
             ease = {{ git = \"{}\", tag = \"v1.1.0\" }}\n",
            ease.display()
        )
    );
    assert!(!w.path("my-game/screens/petal.lock").exists());
    let lock = w.lock("my-game");
    assert_eq!(
        lock.package("util").unwrap().source,
        LockedSource::Path {
            path: "../libs/util".into(),
            within: None
        }
    );
    w.write("my-game/main.ptl", "import util\nimport ease\nprint(twice(cubic(2)))\n");
    assert_eq!(w.ok("my-game", &["run", "main.ptl"]), "16\n");

    // Adding again replaces the entry (here: pinning by rev).
    let rev = w.git("remotes/ease", &["rev-parse", "v1.0.0"]);
    w.ok(
        "my-game",
        &["add", "ease", "--git", ease.to_str().unwrap(), "--rev", &rev],
    );
    assert!(w.read("my-game/petal.toml").contains(&format!("rev = \"{rev}\"")));
    assert_eq!(git_entry(&w.lock("my-game"), "ease").1, rev);

    // A dependency that does not install leaves the manifest alone.
    let before = w.read("my-game/petal.toml");
    let err = w.fails(
        "my-game",
        &["add", "ghost", "--git", ease.to_str().unwrap(), "--tag", "v9.9.9"],
    );
    assert!(err.contains("has no tag 'v9.9.9'"), "{err}");
    assert_eq!(w.read("my-game/petal.toml"), before);
    let err = w.fails("my-game", &["add", "ghost", "--git", "x"]);
    assert!(err.contains("needs --tag <tag> or --rev <rev>"), "{err}");
    let err = w.fails("my-game", &["add", "ghost"]);
    assert!(err.contains("registries are not supported yet"), "{err}");

    // remove prunes the manifest and the lockfile.
    let out = w.ok("my-game", &["remove", "ease"]);
    assert!(out.contains("Removed ease from petal.toml"), "{out}");
    assert!(!w.read("my-game/petal.toml").contains("ease"));
    assert!(w.lock("my-game").package("ease").is_none());
    let err = w.fails("my-game", &["remove", "ease"]);
    assert!(err.contains("has no dependency named 'ease'"), "{err}");

    // Outside any project, the commands say how to start one.
    std::fs::create_dir_all(w.path("nowhere")).unwrap();
    let err = w.fails("nowhere", &["install"]);
    assert!(err.contains("`petal init` starts a project"), "{err}");
}

#[test]
fn a_registry_version_cannot_be_installed_yet() {
    let w = World::new("registry");
    w.write("app/petal.toml", "[package]\nname = \"game\"\n[dependencies]\ngrid = \"0.3\"\n");
    let err = w.fails("app", &["install"]);
    assert!(err.contains("registries are not supported yet"), "{err}");
}

#[test]
fn a_renamed_dependency_is_resolved_and_refused_by_bundle() {
    let w = World::new("renamed");
    w.ease_repo();
    w.write(
        "app/petal.toml",
        &format!(
            "[package]\nname = \"game\"\n[dependencies]\ne = {{ git = \"{}\", tag = \"v1.0.0\" }}\n",
            w.path("remotes/ease").display()
        ),
    );
    w.write("app/main.ptl", "import e\nprint(e.quad(5))\n");
    assert_eq!(w.petal("app", &["run", "main.ptl"]).stdout, "25\n");
    assert_eq!(
        w.lock("app").packages[0].name, "ease",
        "the lockfile names the package, not the key"
    );
    let err = w.fails("app", &["bundle", "main.ptl"]);
    assert!(err.contains("under the name 'e'"), "{err}");
}
