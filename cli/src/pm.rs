//! Running the package-manager commands: `init`, `add`, `remove`,
//! `install`, `update`, `vendor`, `bundle`. Thin shells over `petal-pkgman`;
//! each works on the project of the working directory (the nearest
//! `petal.toml`). Their arguments and help pages are in `petal::cli::pm`.

use std::path::{Path, PathBuf};
use std::process;

use petal::cli::pm::{self, PmCommand};
use petal_pkgman::{self as pkgman, AddSource, InstallOptions, Installed, Update};

/// `petal add`'s source as the package manager takes it.
fn add_source(source: &pm::AddSource) -> AddSource {
    match source {
        pm::AddSource::Path(path) => AddSource::Path(path.clone()),
        pm::AddSource::Git { url, tag, rev } => AddSource::Git {
            url: url.clone(),
            tag: tag.clone(),
            rev: rev.clone(),
        },
    }
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

pub fn handle(command: &PmCommand) {
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
            match pkgman::add(&root, name, &add_source(source)) {
                Ok(done) => {
                    println!("Added {name} to {}", petal::package::MANIFEST_FILE);
                    report(&done);
                }
                Err(e) => die(&format!(
                    "{e}\n{} was left unchanged",
                    petal::package::MANIFEST_FILE
                )),
            }
        }
        PmCommand::Remove { names } => {
            let root = project();
            let mut last = None;
            for name in names {
                match pkgman::remove(&root, name) {
                    Ok(done) => {
                        println!("Removed {name} from {}", petal::package::MANIFEST_FILE);
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
        PmCommand::Bundle {
            entry,
            output,
            no_libraries,
        } => match pkgman::bundle_with(
            Path::new(entry),
            &pkgman::BundleOptions {
                no_libraries: *no_libraries,
            },
        ) {
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

