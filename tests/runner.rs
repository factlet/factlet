use factlet::lisp::test::{TestResult, run_tests, scenario};
use factlet::lisp::{Domain, Program, load};

const SRC: &str = r#"
(unit usd :prefix "$" :places 2)
(enum filing-status single married-joint)
(input filing-status : filing-status)
(collection w2s
  (input wages : usd)
  (input withheld : usd)
  (def rate (/ withheld wages)))
(def total (sum w2s wages))
(def joint? (= filing-status 'married-joint))
"#;

fn program() -> Program {
    load(SRC, &Domain::new()).unwrap()
}

fn run(src: &str) -> Vec<TestResult> {
    run_tests(&program(), "t.lisp", src).unwrap_or_else(|errs| panic!("{errs:?}"))
}

/// Each test's failures, as printed.
fn failures(src: &str) -> Vec<Vec<String>> {
    run(src)
        .iter()
        .map(|r| r.failures.iter().map(|d| d.to_string()).collect())
        .collect()
}

#[test]
fn expectations_pass_and_fail() {
    let results = run(r#"
(test "passes"
  (given filing-status 'married-joint)
  (expect joint? true total ?)
  (member w2s acme (given wages $100 withheld $10) (expect rate 10%))
  (member w2s globex (given wages $50))
  (expect total $150))
(test "fails"
  (empty w2s)
  (expect total $1))
"#);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].name, "passes");
    assert!(results[0].passed(), "{:?}", results[0].failures);
    assert_eq!(results[1].span.line, 8);
    assert_eq!(
        results[1].failures[0].to_string(),
        "t.lisp:10:17: total: expected $1.00, got $0.00"
    );
}

#[test]
fn each_test_starts_from_a_fresh_case() {
    let results = run(r#"
(test "a" (given filing-status 'single) (expect joint? false))
(test "b" (expect joint? ?))
"#);
    assert!(results.iter().all(TestResult::passed), "{results:?}");
}

#[test]
fn errors_are_expected_by_name() {
    assert_eq!(
        failures(
            r#"
(test "zero wages"
  (member w2s a (given wages $0 withheld $0) (expect-error rate))
  (member w2s b (given wages $1 withheld $0) (expect-error rate)))
"#
        ),
        [["t.lisp:4:60: w2s/#b/rate: expected an error, got 0"]]
    );
}

#[test]
fn bad_forms_are_reported_where_written() {
    assert_eq!(
        failures(
            r#"
(test "bad"
  (given nope 1)
  (given filing-status $5)
  (given total $5)
  (expect wages $0)
  (expect w2s ?)
  (member w2s a (member w2s b))
  (member total a)
  (expect total)
  (frobnicate))
"#
        ),
        [[
            "t.lisp:3:10: unknown name `nope`",
            "t.lisp:4:24: filing-status is filing-status, not usd `$5`",
            "t.lisp:5:10: `total` is not an input",
            "t.lisp:6:11: unknown name `wages`",
            "t.lisp:7:11: `w2s` is a collection: use (member w2s …) or (empty w2s)",
            "t.lisp:8:17: collections can't be nested",
            "t.lisp:9:11: `total` is not a collection",
            "t.lisp:10:3: expected name value pairs, found `(expect total)`",
            "t.lisp:11:3: expected given, member, empty, expect or expect-error, found `(frobnicate)`",
        ]]
    );
}

#[test]
fn files_must_hold_only_tests() {
    let errs = run_tests(
        &program(),
        "t.lisp",
        "(given filing-status 'single)\n(test x)",
    )
    .unwrap_err();
    let errs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
    assert_eq!(
        errs,
        [
            "t.lisp:1:1: expected (test \"name\" form …), found `(given filing-status 'single)`",
            "t.lisp:2:1: expected (test \"name\" form …), found `(test x)`",
        ]
    );
    let errs = run_tests(&program(), "t.lisp", "(test \"x\"").unwrap_err();
    assert!(errs[0].to_string().starts_with("t.lisp:"), "{errs:?}");
}

#[test]
fn scenarios_answer_without_expecting() {
    let p = program();
    let mut case = scenario(
        &p,
        "case.lisp",
        "(given filing-status 'single) (member w2s acme (given wages $5))",
    )
    .unwrap();
    assert_eq!(p.get(&mut case, "total").unwrap().to_string(), "$5.00");

    let errs = scenario(&p, "case.lisp", "(expect total $5)")
        .err()
        .unwrap();
    assert_eq!(
        errs[0].to_string(),
        "case.lisp:1:1: expected given, member or empty, found `(expect total $5)`"
    );
}
