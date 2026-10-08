//! The `factlet` command: check, test and run programs from the shell.
//!
//! [`run`] takes the [`Domain`] programs load against, so rules that need
//! their own units or built-in functions can ship the whole command as a
//! binary of their own:
//!
//! ```no_run
//! # fn my_domain() -> factlet::lisp::Domain { factlet::lisp::Domain::new() }
//! fn main() -> std::process::ExitCode {
//!     factlet::cli::run(my_domain())
//! }
//! ```

use crate::Case;
use crate::lisp::test::{run_tests, scenario};
use crate::lisp::{Diagnostic, Domain, Program, Value, load_files};
use clap::{Args, Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "factlet",
    version,
    about = "Check, test and run factlet programs"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Load a program and report every error in it.
    Check {
        /// The main file; includes are found relative to it.
        program: PathBuf,
    },
    /// Run the `(test …)` forms in test files against a program.
    Test {
        program: PathBuf,
        /// Test files, or directories to search for `.lisp` files.
        #[arg(required = true)]
        tests: Vec<PathBuf>,
    },
    /// Print facts' values, and the questions blocking any that are missing.
    Eval {
        program: PathBuf,
        #[command(flatten)]
        answers: Answers,
        #[arg(required = true)]
        facts: Vec<String>,
    },
    /// Print a fact's derivation.
    Explain {
        program: PathBuf,
        #[command(flatten)]
        answers: Answers,
        fact: String,
    },
}

#[derive(Args)]
struct Answers {
    /// A file of `given`, `member` and `empty` forms to answer from.
    #[arg(long = "case", value_name = "FILE")]
    case: Option<PathBuf>,
    /// Answer a global input, as in source: `--set 'wages=$58,000'`.
    #[arg(long = "set", value_name = "NAME=VALUE")]
    set: Vec<String>,
}

/// Run the command on `std::env::args`, loading programs against `domain`.
pub fn run(domain: Domain) -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Check { program } => open(&program, &domain).map(|_| ExitCode::SUCCESS),
        Command::Test { program, tests } => open(&program, &domain).and_then(|p| test(&p, &tests)),
        Command::Eval {
            program,
            answers,
            facts,
        } => open(&program, &domain).and_then(|p| {
            let mut case = answer(&p, &answers)?;
            for name in &facts {
                eval(&p, &mut case, name)?;
            }
            Ok(ExitCode::SUCCESS)
        }),
        Command::Explain {
            program,
            answers,
            fact,
        } => open(&program, &domain).and_then(|p| {
            let mut case = answer(&p, &answers)?;
            let explained = p
                .explain(&mut case, &fact)
                .ok_or_else(|| vec![unknown(&fact)])?;
            print!("{explained}");
            blocked(&p, &mut case, &fact);
            Ok(ExitCode::SUCCESS)
        }),
    };
    result.unwrap_or_else(|errors| {
        for e in errors {
            eprintln!("{e}");
        }
        ExitCode::FAILURE
    })
}

fn error(file: &Path, message: String) -> Diagnostic {
    Diagnostic {
        file: file.display().to_string(),
        span: Default::default(),
        message,
    }
}

fn unknown(name: &str) -> Diagnostic {
    Diagnostic {
        file: String::new(),
        span: Default::default(),
        message: format!("unknown name `{name}`"),
    }
}

fn read(path: &Path) -> Result<String, Vec<Diagnostic>> {
    fs::read_to_string(path).map_err(|e| vec![error(path, e.to_string())])
}

/// Load the program whose main file is `main`; includes are read relative
/// to its directory.
fn open(main: &Path, domain: &Domain) -> Result<Program, Vec<Diagnostic>> {
    let name = main.display().to_string();
    let dir = main.parent().unwrap_or(Path::new(""));
    let read = |file: &str| {
        let path = if file == name {
            main.to_path_buf()
        } else {
            dir.join(file)
        };
        fs::read_to_string(path).ok()
    };
    load_files(&name, read, domain)
}

fn answer(program: &Program, answers: &Answers) -> Result<Case<Value>, Vec<Diagnostic>> {
    let mut case = match &answers.case {
        Some(path) => scenario(program, &path.display().to_string(), &read(path)?)?,
        None => program.case(),
    };
    for set in &answers.set {
        let fail = |message: String| vec![error(Path::new("--set"), message)];
        let (name, value) = set
            .split_once('=')
            .ok_or_else(|| fail(format!("expected NAME=VALUE, found `{set}`")))?;
        let id = program
            .id(name)
            .ok_or_else(|| fail(format!("unknown name `{name}`")))?;
        program
            .set(&mut case, id, value)
            .map_err(|e| fail(format!("{name}: {e}")))?;
    }
    Ok(case)
}

fn eval(program: &Program, case: &mut Case<Value>, name: &str) -> Result<(), Vec<Diagnostic>> {
    let value = program.get(case, name).ok_or_else(|| vec![unknown(name)])?;
    println!("{name} = {value}");
    blocked(program, case, name);
    Ok(())
}

/// Print the questions blocking `name`, if any.
fn blocked(program: &Program, case: &mut Case<Value>, name: &str) {
    let Some(id) = program.id(name) else { return };
    let questions: Vec<String> = program
        .unanswered(case, id)
        .into_iter()
        .map(|f| case.name(f))
        .collect();
    if !questions.is_empty() {
        println!("  blocked on: {}", questions.join(", "));
    }
}

fn test(program: &Program, paths: &[PathBuf]) -> Result<ExitCode, Vec<Diagnostic>> {
    let mut files = Vec::new();
    for path in paths {
        collect(path, &mut files)?;
    }
    let (mut passed, mut failed) = (0, 0);
    for file in &files {
        let name = file.display().to_string();
        let results = match run_tests(program, &name, &read(file)?) {
            Ok(results) => results,
            Err(errors) => {
                println!("FAIL  {name}");
                for e in errors {
                    println!("      {e}");
                }
                failed += 1;
                continue;
            }
        };
        for result in results {
            let at = format!("{name}:{}", result.span.line);
            if result.passed() {
                passed += 1;
                println!("ok    {at}  {}", result.name);
            } else {
                failed += 1;
                println!("FAIL  {at}  {}", result.name);
                for e in &result.failures {
                    println!("      {e}");
                }
            }
        }
    }
    println!("\n{passed} passed, {failed} failed");
    Ok(if failed == 0 && passed > 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// `path`, or every `.lisp` file under it, in name order.
fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), Vec<Diagnostic>> {
    if !path.is_dir() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    let entries = fs::read_dir(path).map_err(|e| vec![error(path, e.to_string())])?;
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
    entries.sort();
    for entry in entries {
        if entry.is_dir() || entry.extension().is_some_and(|e| e == "lisp") {
            collect(&entry, files)?;
        }
    }
    Ok(())
}
