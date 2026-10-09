use petal::cli;

fn main() {
    let args = cli::parse_args();
    cli::execute(args);
}
