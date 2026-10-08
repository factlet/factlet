use factlet::lisp::Domain;
use std::process::ExitCode;

fn main() -> ExitCode {
    factlet::cli::run(Domain::new())
}
