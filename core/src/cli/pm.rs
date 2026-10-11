//! The package-manager commands: `init`, `add`, `remove`, `install`,
//! `update`, `vendor`, `bundle`. Thin shells over [`crate::pkgman`]; each
//! works on the project of the working directory (the nearest `petal.toml`).

use std::path::{Path, PathBuf};
use std::process;

use crate::pkgman::{self, AddSource, InstallOptions, Installed, Update};

use super::{CliArgs, Command, SourceInput};

/// One package-manager invocation.
pub enum PmCommand {
    Init {
        dir: Option<String>,
        name: Option<String>,
    },
    Add {
        name: String,
        source: AddSource,
    },
    Remove {
        names: Vec<String>,
    },
    Install {
        locked: bool,
    },
    Update {
        names: Vec<String>,
    },
    Vendor,
    Bundle {
        entry: String,
        output: Option<String>,
    },
}

fn usage_exit(usage: &str, problem: &str) -> ! {
    eprintln!("{problem}");
    eprintln!("Usage: {usage}");
    process::exit(1);
}

fn wrap(command: PmCommand) -> CliArgs {
    CliArgs {
        command: Command::Pm(command),
        source: SourceInput::Inline(String::new()),
        include_dirs: Vec::new(),
    }
}

/// The value after the flag at `args[*i]`.
fn value<'a>(args: &'a [String], i: &mut usize, usage: &str) -> &'a str {
    let flag = &args[*i];
    *i += 1;
    match args.get(*i) {
        Some(v) => v,
        None => usage_exit(usage, &format!("Expected a value after {flag}")),
    }
}

pub(super) fn parse(command: &str, args: &[String]) -> CliArgs {
    let mut positional: Vec<String> = Vec::new();
    let mut flags: Vec<(String, String)> = Vec::new();
    let mut switches: Vec<String> = Vec::new();
    let (usage, value_flags, switch_flags): (&str, &[&str], &[&str]) = match command {
        "init" => ("petal init [<dir>] [--name <name>]", &["--name"], &[]),
        "add" => (
            "petal add <name> (--path <dir> | --git <url> (--tag <tag> | --rev <rev>))",
            &["--path", "--git", "--tag", "--rev"],
            &[],
        ),
        "remove" => ("petal remove <name>...", &[], &[]),
        "install" => ("petal install [--locked]", &[], &["--locked"]),
        "update" => ("petal update [<name>...]", &[], &[]),
        "vendor" => ("petal vendor", &[], &[]),
        "bundle" => ("petal bundle <entry> [-o <file>]", &["-o", "--output"], &[]),
        other => unreachable!("'{other}' is not a package-manager command"),
    };
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if value_flags.contains(&arg) {
            let v = value(args, &mut i, usage).to_string();
            flags.push((arg.to_string(), v));
        } else if switch_flags.contains(&arg) {
            switches.push(arg.to_string());
        } else if arg.starts_with('-') {
            usage_exit(usage, &format!("Unknown option '{arg}'"));
        } else {
            positional.push(arg.to_string());
        }
        i += 1;
    }
    let flag = |name: &str| {
        flags
            .iter()
            .find(|(f, _)| f == name)
            .map(|(_, v)| v.clone())
    };
    let at_most = |n: usize| {
        if positional.len() > n {
            usage_exit(usage, &format!("Unexpected argument '{}'", positional[n]));
        }
    };
    wrap(match command {
        "init" => {
            at_most(1);
            PmCommand::Init {
                dir: positional.first().cloned(),
                name: flag("--name"),
            }
        }
        "add" => {
            at_most(1);
            let Some(name) = positional.first().cloned() else {
                usage_exit(usage, "Expected the dependency's name");
            };
            let source = match (flag("--path"), flag("--git"), flag("--tag"), flag("--rev")) {
                (Some(path), None, None, None) => AddSource::Path(path),
                (None, Some(url), Some(tag), None) => AddSource::Git {
                    url,
                    tag: Some(tag),
                    rev: None,
                },
                (None, Some(url), None, Some(rev)) => AddSource::Git {
                    url,
                    tag: None,
                    rev: Some(rev),
                },
                (None, None, None, None) => usage_exit(
                    usage,
                    "Say where the package comes from: --path <dir>, or --git <url> with --tag \
                     or --rev (registries are not supported yet)",
                ),
                (None, Some(_), None, None) => {
                    usage_exit(usage, "A git dependency needs --tag <tag> or --rev <rev> to pin it")
                }
                _ => usage_exit(
                    usage,
                    "Give either --path, or --git with exactly one of --tag and --rev",
                ),
            };
            PmCommand::Add { name, source }
        }
        "remove" => {
            if positional.is_empty() {
                usage_exit(usage, "Expected the name of a dependency to remove");
            }
            PmCommand::Remove { names: positional }
        }
        "install" => {
            at_most(0);
            PmCommand::Install {
                locked: !switches.is_empty(),
            }
        }
        "update" => PmCommand::Update { names: positional },
        "vendor" => {
            at_most(0);
            PmCommand::Vendor
        }
        "bundle" => {
            at_most(1);
            let Some(entry) = positional.first().cloned() else {
                usage_exit(usage, "Expected the entry file of the program to bundle");
            };
            PmCommand::Bundle {
                entry,
                output: flag("-o").or_else(|| flag("--output")),
            }
        }
        _ => unreachable!(),
    })
}

fn die(message: &str) -> ! {
    eprintln!("Error: {message}");
    process::exit(1);
}

fn here() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn project() -> PathBuf {
    pkgman::project_root(&here()).unwrap_or_else(|e| die(&e))
}

/// Say what an install left behind.
fn report(done: &Installed) {
    let count = done.graph.dependencies().len();
    let packages = if count == 1 { "package" } else { "packages" };
    if done.lock_changed {
        println!("Locked {count} {packages} in {}", pkgman::LOCK_FILE);
    } else {
        println!("{} is up to date ({count} {packages})", pkgman::LOCK_FILE);
    }
    for node in done.graph.dependencies() {
        println!(
            "    {}{}  {}",
            node.name,
            node.version
                .as_deref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default(),
            node.source
        );
    }
    if !done.vendored.is_empty() {
        println!(
            "Vendored into {}/: {}",
            pkgman::VENDOR_DIR,
            done.vendored.join(", ")
        );
    }
}

pub(super) fn handle(command: &PmCommand) {
    match command {
        PmCommand::Init { dir, name } => {
            let dir = dir.as_deref().map(PathBuf::from).unwrap_or_else(here);
            match pkgman::init(&dir, name.as_deref()) {
                Ok(path) => println!("Created {}", path.display()),
                Err(e) => die(&e),
            }
        }
        PmCommand::Add { name, source } => {
            let root = project();
            match pkgman::add(&root, name, source) {
                Ok(done) => {
                    println!("Added {name} to {}", crate::package::MANIFEST_FILE);
                    report(&done);
                }
                Err(e) => die(&format!(
                    "{e}\n{} was left unchanged",
                    crate::package::MANIFEST_FILE
                )),
            }
        }
        PmCommand::Remove { names } => {
            let root = project();
            let mut last = None;
            for name in names {
                match pkgman::remove(&root, name) {
                    Ok(done) => {
                        println!("Removed {name} from {}", crate::package::MANIFEST_FILE);
                        last = Some(done);
                    }
                    Err(e) => die(&e),
                }
            }
            if let Some(done) = last {
                report(&done);
            }
        }
        PmCommand::Install { locked } => install(InstallOptions {
            locked: *locked,
            announce: true,
            ..InstallOptions::default()
        }),
        PmCommand::Update { names } => {
            let root = project();
            // A name that is not in the graph is a typo, not a no-op.
            if !names.is_empty()
                && let Ok(Some(lock)) = pkgman::Lock::load(&root)
                && let Some(unknown) = names.iter().find(|n| lock.package(n).is_none())
            {
                die(&format!(
                    "{} pins no package named '{unknown}'",
                    pkgman::LOCK_FILE
                ));
            }
            install(InstallOptions {
                update: if names.is_empty() {
                    Update::All
                } else {
                    Update::Names(names.clone())
                },
                announce: true,
                ..InstallOptions::default()
            })
        }
        PmCommand::Vendor => install(InstallOptions {
            vendor: true,
            announce: true,
            ..InstallOptions::default()
        }),
        PmCommand::Bundle { entry, output } => match pkgman::bundle(Path::new(entry)) {
            Ok(bundle) => {
                let text = serde_json::to_string_pretty(&bundle).expect("a bundle is plain JSON");
                match output {
                    Some(path) => {
                        if let Err(e) = std::fs::write(path, text + "\n") {
                            die(&format!("cannot write {path}: {e}"));
                        }
                    }
                    None => println!("{text}"),
                }
            }
            Err(e) => die(&e),
        },
    }
}

fn install(options: InstallOptions) {
    match pkgman::install_with(&project(), &options) {
        Ok(done) => report(&done),
        Err(e) => die(&e),
    }
}

// ── help pages ───────────────────────────────────────────────────

pub(super) const INIT: &str = "\
NAME
       petal-init - Start a project: write a petal.toml

SYNOPSIS
       petal init [<dir>] [--name <name>]

DESCRIPTION
       Writes a petal.toml with a [package] section into <dir> (the working
       directory when absent), creating the directory if needed. The package
       name is --name, or the directory's name bent into an identifier. An
       existing petal.toml is never overwritten.

       Every command that compiles finds the nearest petal.toml above the
       file it is given, so once a directory has one its scripts can import
       the project's dependencies with no flag.

SEE ALSO
       petal help add, petal help install, docs/packages.md
";

pub(super) const ADD: &str = "\
NAME
       petal-add - Add a dependency to the project and install it

SYNOPSIS
       petal add <name> --path <dir>
       petal add <name> --git <url> --tag <tag>
       petal add <name> --git <url> --rev <rev>

DESCRIPTION
       Writes the dependency into the [dependencies] section of the project's
       petal.toml (the nearest one at or above the working directory),
       replacing an entry of the same name, then installs: the package is
       fetched and petal.lock is updated. <name> is the import name - what
       scripts write after 'import'. If the install fails, petal.toml is put
       back as it was.

       --path <dir>
              A package directory, relative to the project root.

       --git <url> with --tag <tag> or --rev <rev>
              A git repository holding a petal.toml at its root, pinned to a
              tag or a commit.

       A registry version (name = \"0.3\") cannot be added: registries are not
       supported yet.

SEE ALSO
       petal help remove, petal help install, petal help packages,
       docs/packages.md
";

pub(super) const REMOVE: &str = "\
NAME
       petal-remove - Remove a dependency from the project

SYNOPSIS
       petal remove <name>...

DESCRIPTION
       Deletes each named entry from [dependencies] in the project's
       petal.toml and installs again, which drops from petal.lock every
       package nothing depends on any more. A vendor/<name> directory is
       left for the user to delete.

SEE ALSO
       petal help add, petal help install
";

pub(super) const INSTALL: &str = "\
NAME
       petal-install - Fetch the project's dependencies and write petal.lock

SYNOPSIS
       petal install [--locked]

DESCRIPTION
       Resolves the dependency graph of the project (the nearest petal.toml
       at or above the working directory) into exactly one version of each
       package, fetches what is missing, and writes petal.lock beside
       petal.toml.

       Git dependencies are fetched with git into a cache under ~/.petal/cache
       (set PETAL_HOME to move ~/.petal), each checked-out tree stored by a
       hash of its contents. petal.lock pins every package's source, the
       commit its tag or rev resolved to, and that tree hash. A later install
       reuses the locked commit, and what it fetches - and every vendored or
       cached tree already there - is verified against the locked hash.

       When two packages depend on the same repository at different tags,
       the higher tag is used (minimal version selection: the highest minimum
       anyone asks for; no solver). Pins that cannot be ordered - two
       different revs, or a tag and a different commit - are an error, as are
       two different sources for one package name.

       A vendor/<name> directory at the project root is used in preference to
       the cache. When vendor/ exists, install keeps the packages already in
       it in step with petal.lock.

       'petal check' and 'petal run' install on demand: the first time a
       program imports a git dependency that is not installed, this command
       runs for its project. Set PETAL_OFFLINE=1 to forbid fetching.

OPTIONS
       --locked
              Fail instead of changing petal.lock - for CI, where the
              lockfile in the repository is the truth.

SEE ALSO
       petal help update, petal help vendor, petal help packages,
       docs/packages.md
";

pub(super) const UPDATE: &str = "\
NAME
       petal-update - Re-resolve locked dependencies from their sources

SYNOPSIS
       petal update [<name>...]

DESCRIPTION
       Like 'petal install', but the named packages (every package when none
       is named) are resolved afresh instead of from petal.lock: a tag is
       looked up again in the repository, and the lockfile takes the commit
       and tree hash found now. Use it after changing a tag in petal.toml is
       not enough - when a tag was moved upstream, or a locked tree no longer
       verifies and the change is expected.

SEE ALSO
       petal help install
";

pub(super) const VENDOR: &str = "\
NAME
       petal-vendor - Copy the project's git dependencies into vendor/

SYNOPSIS
       petal vendor

DESCRIPTION
       Installs, then copies each git package of the locked graph into
       vendor/<name> at the project root. Every command uses vendor/<name> in
       preference to the cache, so a project with its vendor/ directory
       checked in builds with no network and no cache. Path dependencies are
       already on disk and are not copied.

       A vendored tree is verified against petal.lock by 'petal install'; one
       that was edited by hand fails that check, and running 'petal vendor'
       again rewrites it from the locked source.

       A vendor/<name> directory holding a petal.toml is also found by
       'import <name>' when no manifest lists the name, ahead of the search
       path and the copy bundled in the binary.

SEE ALSO
       petal help install, petal help bundle
";

pub(super) const BUNDLE: &str = "\
NAME
       petal-bundle - Emit a program's packages as one JSON document

SYNOPSIS
       petal bundle <entry> [-o <file>]

DESCRIPTION
       Prints every module of the project's installed dependency graph - the
       project's own package included - in the in-memory form
       register_package accepts, for a host with no filesystem (a wasm page,
       an embedded device). Installs first if something is missing.

       The document has: format (1); entry, with the entry file's name, its
       source, and the path to load it at so its relative imports resolve
       inside the project's in-memory package (null when the entry is not
       under the project's module directory); packages, each with name,
       version and modules - a map of module name to source that also carries
       the package's petal.toml; and requires, the host interfaces the graph
       needs.

       A host registers each package (Env::register_package,
       pb_vm_register_package, or the wasm register_package(name, json)) and
       loads entry.source at entry.path. Packages bundled in the petal binary
       (ui, bloom, the builtin packages) are not repeated in the document.

OPTIONS
       -o <file>, --output <file>
              Write the document to <file> instead of stdout.

SEE ALSO
       petal help vendor, docs/packages.md, docs/embedding-guide.md
";
