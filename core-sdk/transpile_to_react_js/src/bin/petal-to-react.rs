//! `petal-to-react <file.ptl | folder> [out-dir] [--dialect design|web|generic|immediate] [--name Component]`
//!
//! Prints the React module for one file, or with an output folder writes
//! every module the entry reaches, the runtime files they import, and an
//! elements stub for tags no dialect knows. Warnings go to stderr.

use std::path::PathBuf;
use std::process::ExitCode;

use transpile_to_react_js::{Dialect, Options, transpile_project};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut input: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut opts = Options::default();
    let mut strict = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dialect" => {
                let d = args.next().unwrap_or_default();
                match Dialect::parse(&d) {
                    Some(d) => opts.dialect = Some(d),
                    None => {
                        eprintln!("unknown dialect `{d}` (design, web, generic, immediate)");
                        return ExitCode::from(2);
                    }
                }
            }
            "--name" => opts.component_name = args.next(),
            "--strict" => strict = true,
            "-h" | "--help" => {
                eprintln!("usage: petal-to-react <file.ptl | folder> [out-dir] [--dialect design|web|generic|immediate] [--name Component] [--strict]");
                return ExitCode::SUCCESS;
            }
            _ if input.is_none() => input = Some(a.into()),
            _ if out.is_none() => out = Some(a.into()),
            _ => {
                eprintln!("unexpected argument `{a}`");
                return ExitCode::from(2);
            }
        }
    }
    let Some(input) = input else {
        eprintln!("usage: petal-to-react <file.ptl | folder> [out-dir]");
        return ExitCode::from(2);
    };
    let files = match transpile_project(&input, &opts) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut warned = 0;
    for f in &files {
        for w in &f.output.warnings {
            eprintln!("{}:{w}", f.source.display());
            warned += 1;
        }
    }
    match out {
        None => {
            for f in &files {
                if files.len() > 1 {
                    println!("// ── {} ──", f.out.display());
                }
                print!("{}", f.output.code);
            }
        }
        Some(dir) => {
            let mut runtime = std::collections::BTreeMap::new();
            let mut elements = std::collections::BTreeSet::new();
            for f in &files {
                let p = dir.join(&f.out);
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&p, &f.output.code) {
                    eprintln!("error: {}: {e}", p.display());
                    return ExitCode::FAILURE;
                }
                eprintln!("wrote {}", p.display());
                for (n, c) in &f.output.runtime {
                    runtime.insert(*n, *c);
                }
                elements.extend(f.output.elements.iter().cloned());
            }
            for (n, c) in runtime {
                let p = dir.join(n);
                let _ = std::fs::write(&p, c);
                eprintln!("wrote {}", p.display());
            }
            let el = dir.join("elements.jsx");
            if !elements.is_empty() && !el.exists() {
                let mut s = String::from("// Components for the tags of the Petal program that are not HTML.\n// Generated as stubs by petal-to-react: replace each with the app's own.\n\n");
                for e in &elements {
                    s.push_str(&format!("export function {e}({{ children, ...props }}) {{\n  return <div data-petal-tag=\"{e}\" {{...props}}>{{children}}</div>;\n}}\n\n"));
                }
                let _ = std::fs::write(&el, s);
                eprintln!("wrote {} (stubs)", el.display());
            }
        }
    }
    if warned > 0 {
        eprintln!("{warned} warning(s)");
        if strict {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
