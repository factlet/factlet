//! The `factlet` command against `tests/fixtures`.
#![cfg(feature = "cli")]

use std::process::Command;

/// Run `factlet args…` from the crate root: (exit code, stdout, stderr).
fn factlet(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_factlet"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (
        out.status.code().unwrap(),
        text(out.stdout),
        text(out.stderr),
    )
}

const MAIN: &str = "tests/fixtures/main.lisp";

#[test]
fn check_reports_errors_with_their_file() {
    assert_eq!(factlet(&["check", MAIN]), (0, String::new(), String::new()));
    let (code, _, err) = factlet(&["check", "tests/fixtures/law.lisp.missing"]);
    assert_eq!(code, 1);
    assert!(err.starts_with("tests/fixtures/law.lisp.missing"), "{err}");
}

#[test]
fn test_runs_directories_and_reports_failures() {
    let (code, out, _) = factlet(&["test", MAIN, "tests/fixtures/tests"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("ok    tests/fixtures/tests/pass.lisp:1  single, one W-2"),
        "{out}"
    );
    assert!(out.ends_with("3 passed, 0 failed\n"), "{out}");

    let (code, out, _) = factlet(&["test", MAIN, "tests/fixtures/broken/fail.lisp"]);
    assert_eq!(code, 1);
    assert!(
        out.contains("FAIL  tests/fixtures/broken/fail.lisp:1  wrong answer"),
        "{out}"
    );
    assert!(
        out.contains(
            "tests/fixtures/broken/fail.lisp:4:26: taxable-income: expected $1.00, got $0.00"
        ),
        "{out}"
    );
    assert!(out.ends_with("0 passed, 2 failed\n"), "{out}");
}

#[test]
fn eval_answers_from_a_case_and_flags() {
    let (code, out, _) = factlet(&["eval", MAIN, "taxable-income"]);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "taxable-income = ?\n  blocked on: filing-status, w2s\n"
    );

    let (code, out, _) = factlet(&[
        "eval",
        MAIN,
        "--case",
        "tests/fixtures/case.lisp",
        "--set",
        "interest=$100",
        "taxable-income",
    ]);
    assert_eq!((code, out.as_str()), (0, "taxable-income = $42,350.00\n"));

    let (code, _, err) = factlet(&["eval", MAIN, "--set", "interest=5", "taxable-income"]);
    assert_eq!(
        (code, err.as_str()),
        (1, "--set: interest: interest is usd, not number `5`\n")
    );
}

#[test]
fn explain_prints_the_derivation() {
    let (code, out, _) = factlet(&[
        "explain",
        MAIN,
        "--case",
        "tests/fixtures/case.lisp",
        "taxable-income",
    ]);
    assert_eq!(code, 0);
    assert!(
        out.starts_with("taxable-income = $42,250.00  ; Taxable income (line 15)\n"),
        "{out}"
    );
    assert!(
        out.contains("law/deduction = $15,750.00  ; IRC §63(c)(2)"),
        "{out}"
    );

    let (code, _, err) = factlet(&["explain", MAIN, "nope"]);
    assert_eq!((code, err.as_str()), (1, "unknown name `nope`\n"));
}

#[test]
fn form1040_runs_on_the_default_domain() {
    let program = "examples/form1040/form1040.lisp";
    assert_eq!(factlet(&["check", program]).0, 0);
    let (code, out, _) = factlet(&["test", program, "examples/form1040/tests.lisp"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.ends_with("4 passed, 0 failed\n"), "{out}");
}
