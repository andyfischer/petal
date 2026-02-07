use std::fs;
use std::process;

use petal::env::Env;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: petal <file.ptl>");
        eprintln!("       petal -e <code>");
        process::exit(1);
    }

    let source = if args[1] == "-e" {
        if args.len() < 3 {
            eprintln!("Usage: petal -e <code>");
            process::exit(1);
        }
        args[2].clone()
    } else {
        match fs::read_to_string(&args[1]) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading file '{}': {}", args[1], e);
                process::exit(1);
            }
        }
    };

    if let Err(e) = run(&source) {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn run(source: &str) -> Result<(), String> {
    let mut env = Env::new();
    env.run_source(source)?;
    Ok(())
}
