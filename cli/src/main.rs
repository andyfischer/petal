//! The `petal` binary: the command-line interface in `petal::cli`, with the
//! package manager (`petal-pkgman`) registered. The package manager is not
//! part of the `petal` library, so the runtimes embedded in other programs —
//! the browser, SDL and C hosts — do not carry it; this binary is where it
//! comes in.

mod pm;

use petal::cli::{self, Command};

fn main() {
    // Git dependencies resolve through it for every Env this process makes:
    // the commands that compile, the language server, the test runner.
    petal_pkgman::register();
    let args = cli::parse_args();
    if let Command::Pm(command) = &args.command {
        pm::handle(command);
        return;
    }
    cli::execute(args);
}
