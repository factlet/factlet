use factlet::Case;
use factlet::lisp::{Domain, Program, Value, load};

/// The self-employed health insurance deduction and the premium tax
/// credit, each depending on the other through AGI (Rev. Proc. 2014-41).
/// Simplified: the credit is the benchmark less 8.5% of AGI.
const SEHI_PTC: &str = r#"
(unit usd :prefix "$" :places 2)
(input wages : usd)
(input se-profit : usd)
(input premiums : usd)
(input benchmark : usd)
(input other-income : usd)

(defn excess [a : usd b : usd] (max $0 (- a b)))

(fixpoint sch1/line17 :start $0 :within $1)
(def sch1/line17 (min premiums-net se-profit))
(def premiums-net (excess premiums ptc))
(def line11/agi (- (+ wages se-profit) sch1/line17))
(def ptc (min premiums (excess benchmark (* 8.5% line11/agi))))

(def total-income (+ line11/agi other-income))
"#;

fn program(src: &str) -> Program {
    load(src, &Domain::new()).unwrap_or_else(|errs| {
        let errs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        panic!("{}", errs.join("\n"))
    })
}

fn errors(src: &str) -> Vec<String> {
    match load(src, &Domain::new()) {
        Ok(_) => panic!("loaded"),
        Err(errs) => errs.iter().map(|e| e.to_string()).collect(),
    }
}

fn get(p: &Program, c: &mut Case<Value>, name: &str) -> String {
    p.get(c, name).unwrap().to_string()
}

fn answered(p: &Program) -> Case<Value> {
    let mut c = p.case();
    for (name, value) in [
        ("wages", "$0"),
        ("se-profit", "$50,000"),
        ("premiums", "$12,000"),
        ("benchmark", "$10,000"),
        ("other-income", "$0"),
    ] {
        p.set(&mut c, p.id(name).unwrap(), value).unwrap();
    }
    c
}

#[test]
fn iterates_until_within_a_dollar() {
    // Closed form: D = $12,000 - ($5,750 + 8.5% D), so D = $6,250 / 1.085
    // = $5,760.37. From D = $0 the rounds give $6,250, $5,718.75,
    // $5,763.91, $5,760.07, then $5,760.39, which moved by under $1.
    let p = program(SEHI_PTC);
    let mut c = answered(&p);
    assert_eq!(get(&p, &mut c, "sch1/line17"), "$5,760.39");
    assert_eq!(get(&p, &mut c, "line11/agi"), "$44,239.61");
    assert_eq!(get(&p, &mut c, "ptc"), "$6,239.63");
    assert_eq!(get(&p, &mut c, "fixpoint/sch1/line17/rounds"), "5");
    // AGI is $50,000 less the final deduction, and the credit follows
    // that AGI; both are within $1 of the closed form.
    assert_eq!(get(&p, &mut c, "premiums-net"), "$5,760.37");
    c.check_invariants();
}

#[test]
fn recomputes_only_when_the_loop_is_touched() {
    let p = program(SEHI_PTC);
    let mut c = answered(&p);
    assert_eq!(get(&p, &mut c, "total-income"), "$44,239.61");

    c.reset_stats();
    p.set(&mut c, p.id("other-income").unwrap(), "$100")
        .unwrap();
    assert_eq!(get(&p, &mut c, "total-income"), "$44,339.61");
    assert_eq!(c.stats().executed, 1, "the loop isn't rerun");

    c.reset_stats();
    p.set(&mut c, p.id("premiums").unwrap(), "$11,000").unwrap();
    // Closed form: ($11,000 - $5,750) / 1.085 = $4,838.71.
    assert_eq!(get(&p, &mut c, "sch1/line17"), "$4,838.73");
    assert_eq!(get(&p, &mut c, "total-income"), "$45,261.27");
    let s = c.stats();
    assert_eq!(s.executed, 4, "the loop, line 17, AGI, total income: {s:?}");
    c.check_invariants();
}

#[test]
fn missing_answers_stop_the_loop() {
    let p = program(SEHI_PTC);
    let mut c = p.case();
    for (name, value) in [
        ("wages", "$0"),
        ("se-profit", "$50,000"),
        ("benchmark", "$10,000"),
    ] {
        p.set(&mut c, p.id(name).unwrap(), value).unwrap();
    }
    assert_eq!(get(&p, &mut c, "line11/agi"), "?");
    let agi = p.id("line11/agi").unwrap();
    let names: Vec<String> = c.unanswered(agi).into_iter().map(|f| c.name(f)).collect();
    assert_eq!(names, ["premiums"]);
}

#[test]
fn explain_goes_through_the_loop() {
    let p = program(SEHI_PTC);
    let mut c = answered(&p);
    let tree = p.explain(&mut c, "line11/agi").unwrap().to_string();
    let lines: Vec<&str> = tree.lines().take(3).collect();
    assert_eq!(
        lines,
        [
            "line11/agi = $44,239.61",
            "└── fixpoint/sch1/line17 = [$44,239.61 $6,239.63 $5,760.37 $5,760.39 5]",
            "    ├── wages = $0.00",
        ]
    );
}

#[test]
fn oscillation_never_converges() {
    let p = program("(fixpoint x :start 0 :max 10)\n(def x (- 100 x))\n(def y (+ x 1))");
    let mut c = p.case();
    assert_eq!(
        get(&p, &mut c, "x"),
        "<error: no convergence after 10 rounds>"
    );
    assert_eq!(
        get(&p, &mut c, "y"),
        "<error: no convergence after 10 rounds>"
    );
}

#[test]
fn diagnostics() {
    assert_eq!(
        errors("(def a (+ b 1))\n(def b (+ a 1))"),
        ["2:11: cycle: a -> b -> a; break it with (fixpoint a :start …)"]
    );
    assert_eq!(
        errors("(def a 1)\n(fixpoint a :start 0)"),
        ["2:11: `a` isn't in a cycle"]
    );
    assert_eq!(
        errors("(input a : number)\n(fixpoint a :start 0)"),
        ["2:11: fixpoint `a` names no def"]
    );
    assert_eq!(
        errors("(fixpoint a :start true)\n(def a (+ b 1))\n(def b (if a 1 2))"),
        ["2:8: fixpoint `a` starts as bool but is computed as number"]
    );
    assert_eq!(
        errors("(fixpoint a :start 0)\n(def a (+ b 1))\n(def b (+ a c))\n(def c (+ b 1))"),
        ["1:11: the cycle through a, b, c needs more fixpoints to break it"]
    );
    assert_eq!(
        errors(
            "(defn f [x : number] (+ x a))\n(fixpoint a :start 0)\n(def a (f b))\n(def b (+ a 1))"
        ),
        ["1:27: `a` is in a fixpoint cycle; pass it to functions as an argument"]
    );
    assert_eq!(errors("(fixpoint a)"), ["1:1: fixpoint `a` needs a :start"]);
}
