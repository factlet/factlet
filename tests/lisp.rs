use factlet::Case;
use factlet::lisp::{Domain, Program, Value, load};

const USD: &str = "(unit usd :prefix \"$\" :places 2)\n";

fn program(src: &str) -> Program {
    load(&format!("{USD}{src}"), &Domain::new()).unwrap_or_else(|errs| {
        let errs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        panic!("{}", errs.join("\n"))
    })
}

/// Every diagnostic, with line numbers counted after the `usd` prelude.
fn errors(src: &str) -> Vec<String> {
    match load(&format!("{USD}{src}"), &Domain::new()) {
        Ok(_) => panic!("loaded"),
        Err(errs) => errs
            .iter()
            .map(|e| format!("{}:{}: {}", e.span.line - 1, e.span.col, e.message))
            .collect(),
    }
}

fn get(p: &Program, c: &mut Case<Value>, name: &str) -> String {
    p.get(c, name).unwrap().to_string()
}

#[test]
fn global_rules_evaluate_and_fold() {
    let p = program(
        "(def taxable (max $0 (- wages law/deduction)))  ; any order
         (input wages : usd)
         (def law/deduction (+ law/base law/bonus))
         (def law/base $15,000)
         (def law/bonus $750)
         (def tax (round $1 (* 10% taxable)))",
    );
    let g = &p.graph;
    assert_eq!(g.folded(), 3);
    assert!(g.is_folded(p.id("law/deduction").unwrap()));

    let mut c = p.case();
    assert_eq!(get(&p, &mut c, "tax"), "?");
    let wages = p.id("wages").unwrap();
    assert_eq!(c.unanswered(p.id("tax").unwrap()), vec![wages]);

    p.set(&mut c, wages, "$20,000.49").unwrap();
    assert_eq!(get(&p, &mut c, "taxable"), "$4,250.49");
    assert_eq!(get(&p, &mut c, "tax"), "$425.00");

    // Still clamped at zero: taxable is backdated and tax never reruns.
    p.set(&mut c, wages, "$1").unwrap();
    assert_eq!(get(&p, &mut c, "tax"), "$0.00");
    c.reset_stats();
    p.set(&mut c, wages, "$2").unwrap();
    assert_eq!(get(&p, &mut c, "tax"), "$0.00");
    assert_eq!((c.stats().executed, c.stats().backdated), (1, 1));
    c.check_invariants();
}

#[test]
fn branches_not_taken_are_not_read() {
    let p = program(
        "(input over-65 : bool)
         (input age-credit : usd)
         (def credit (if over-65 age-credit $0))
         (def label (cond ((given? age-credit) \"claimed\") (else \"none\")))
         (def credit-or-zero (or-else age-credit $0))
         (def both (and over-65 (> age-credit $100)))",
    );
    let mut c = p.case();
    let (over, credit) = (p.id("over-65").unwrap(), p.id("credit").unwrap());
    assert_eq!(c.unanswered(credit), vec![over]);
    p.set(&mut c, over, "false").unwrap();
    assert_eq!(get(&p, &mut c, "credit"), "$0.00");
    assert!(c.unanswered(credit).is_empty(), "age-credit isn't read");

    assert_eq!(get(&p, &mut c, "label"), "\"none\"");
    assert_eq!(get(&p, &mut c, "credit-or-zero"), "$0.00");
    assert_eq!(
        get(&p, &mut c, "both"),
        "false",
        "false and missing is false"
    );

    p.set(&mut c, over, "true").unwrap();
    assert_eq!(get(&p, &mut c, "both"), "?");
    p.set(&mut c, p.id("age-credit").unwrap(), "$500").unwrap();
    assert_eq!(get(&p, &mut c, "credit"), "$500.00");
    assert_eq!(get(&p, &mut c, "both"), "true");
    assert_eq!(get(&p, &mut c, "label"), "\"claimed\"");
}

#[test]
fn let_and_arithmetic() {
    let p = program(
        "(input a : usd)
         (def ratio (let [half (/ a 2) third (/ a 3)] (/ half third)))
         (def neg (abs (- a)))
         (def steps (floor $100 (* a 1.5)))
         (def broken (/ a (- a a)))",
    );
    let mut c = p.case();
    p.set(&mut c, p.id("a").unwrap(), "$1,000").unwrap();
    assert_eq!(get(&p, &mut c, "ratio"), "1.5");
    assert_eq!(get(&p, &mut c, "neg"), "$1,000.00");
    assert_eq!(get(&p, &mut c, "steps"), "$1,500.00");
    assert_eq!(get(&p, &mut c, "broken"), "<error: division by zero>");
}

#[test]
fn enums_and_tables() {
    let p = program(
        "(enum status single joint hoh)
         (input status : status)
         (def deduction (table status single $15,750 (joint) $31,500 hoh $23,625))
         (def joint? (= status 'joint))
         (def flat (table status joint $2 else $1))",
    );
    let mut c = p.case();
    let status = p.id("status").unwrap();
    assert_eq!(get(&p, &mut c, "deduction"), "?");
    p.set(&mut c, status, "'hoh").unwrap();
    assert_eq!(get(&p, &mut c, "deduction"), "$23,625.00");
    assert_eq!(get(&p, &mut c, "joint?"), "false");
    assert_eq!(get(&p, &mut c, "flat"), "$1.00");
    p.set(&mut c, status, "joint").unwrap();
    assert_eq!(get(&p, &mut c, "flat"), "$2.00");

    assert_eq!(
        p.set(&mut c, status, "$5"),
        Err("status is status, not usd `$5`".into())
    );
    assert_eq!(
        p.set(&mut c, status, "married"),
        Err("unknown variant `married`".into())
    );
}

#[test]
fn collections() {
    let p = program(
        "(input rate : number)
         (collection w2s
           (input wages : usd)
           (input withheld : usd)
           (def taxed (* wages rate)))
         (def wages (sum w2s wages))
         (def taxed (sum w2s taxed))
         (def employers (count w2s))
         (def big (count w2s (> wages $50,000)))
         (def any-withheld (any w2s (> withheld $0)))
         (def largest (max-of w2s wages))",
    );
    let mut c = p.case();
    let w2s = p.id("w2s").unwrap();
    let wages = p.id("w2s/*/wages").unwrap();
    let withheld = p.id("w2s/*/withheld").unwrap();
    assert_eq!(get(&p, &mut c, "wages"), "?");
    assert_eq!(c.unanswered(p.id("wages").unwrap()), vec![w2s]);

    c.set_empty(w2s).unwrap();
    assert_eq!(get(&p, &mut c, "wages"), "$0.00");
    assert_eq!(get(&p, &mut c, "employers"), "0");
    assert_eq!(get(&p, &mut c, "largest"), "?");

    let acme = c.add_member(w2s, "acme").unwrap();
    let globex = c.add_member(w2s, "globex").unwrap();
    let names: Vec<String> = c
        .unanswered(p.id("wages").unwrap())
        .into_iter()
        .map(|f| c.name(f))
        .collect();
    assert_eq!(names, ["w2s/#acme/wages", "w2s/#globex/wages"]);

    p.set(&mut c, (wages, acme), "$60,000").unwrap();
    p.set(&mut c, (wages, globex), "$20,000").unwrap();
    p.set(&mut c, p.id("rate").unwrap(), "10%").unwrap();
    assert_eq!(get(&p, &mut c, "wages"), "$80,000.00");
    assert_eq!(get(&p, &mut c, "taxed"), "$8,000.00");
    assert_eq!(get(&p, &mut c, "employers"), "2");
    assert_eq!(get(&p, &mut c, "big"), "1");
    assert_eq!(get(&p, &mut c, "largest"), "$60,000.00");
    assert_eq!(get(&p, &mut c, "any-withheld"), "?");
    p.set(&mut c, (withheld, globex), "$10").unwrap();
    assert_eq!(get(&p, &mut c, "any-withheld"), "true", "decided by globex");

    // One member's edit reruns its own field rule, not the other's.
    c.reset_stats();
    p.set(&mut c, (wages, acme), "$70,000").unwrap();
    assert_eq!(get(&p, &mut c, "taxed"), "$9,000.00");
    assert_eq!(c.stats().executed, 2);
    c.check_invariants();
}

#[test]
fn explain_reads_like_source() {
    let p = program(
        "(input wages : usd)
         (def law/deduction $15,750)
         (def taxable (max $0 (- wages law/deduction)))",
    );
    let mut c = p.case();
    p.set(&mut c, p.id("wages").unwrap(), "$20,000").unwrap();
    assert_eq!(
        p.explain(&mut c, "taxable").unwrap().to_string(),
        "\
taxable = $4,250.00
├── wages = $20,000.00
└── law/deduction = $15,750.00
"
    );
}

#[test]
fn diagnostics() {
    assert_eq!(errors("(def x (+ y 1))"), ["1:11: unknown name `y`"]);
    assert_eq!(
        errors("(input a : usd)\n(def x (+ a 1))"),
        ["2:8: `+` can't combine usd and a plain number"]
    );
    assert_eq!(
        errors("(input a : usd)\n(def x (* a a))"),
        ["2:8: `*` can't combine usd and usd"]
    );
    assert_eq!(
        errors("(input a : usd)\n(def x (if a 1 2))"),
        ["2:12: expected bool, found usd `a`"]
    );
    assert_eq!(
        errors("(def x (round $1))"),
        ["1:8: `round` takes 2 arguments"]
    );
    assert_eq!(
        errors("(def a (+ b 1))\n(def b (+ c 1))\n(def c (+ a 1))"),
        ["3:11: cycle: a -> b -> c -> a; break it with (fixpoint a :start …)"]
    );
    assert_eq!(errors("(input a : money)"), ["1:12: unknown type `money`"]);
    assert_eq!(
        errors("(input a : usd)\n(def a 1)"),
        ["2:6: `a` is defined twice"]
    );
    assert_eq!(
        errors("(enum s x y z)\n(input s : s)\n(def d (table s x 1 y 2))"),
        ["3:8: `table` doesn't cover z"]
    );
    assert_eq!(
        errors("(collection c (input f : usd))\n(def x (+ c 1))\n(def y f)"),
        [
            "2:11: `c` is a collection; use (sum c …), (count c) and so on",
            "3:8: `f` is a field of `c`; read it inside (sum c …) or a rule of `c`",
        ]
    );
    assert_eq!(
        errors("(def x (frobnicate 1))"),
        ["1:8: unknown function `frobnicate`"]
    );
    assert_eq!(
        errors("(def x $1,00)"),
        ["1:8: bad number `$1,00`: `,` must separate groups of three digits"]
    );
    assert_eq!(errors("(def x £5)"), ["1:8: `£5`: unknown unit prefix `£`"]);
    assert_eq!(
        errors("(enum s x y)\n(def d x)"),
        ["2:8: unknown name `x`; for the variant, write 'x"]
    );
    assert_eq!(
        errors("(def x (cond ((> 1 2) 3)))"),
        ["1:8: `cond` needs a final (else value)"]
    );
}

#[test]
fn functions() {
    let p = program(
        "(defn at-least-zero [x : usd] (max $0 x))
         (defn phase-out [amount : usd income : usd limit : usd rate : number]
           (at-least-zero (- amount (* rate (at-least-zero (- income limit))))))
         (input income : usd)
         (collection kids (input age : number))
         (defn credit-for [age : number] (if (< age 17) $2,000 $500))
         (def credit (phase-out (sum kids (credit-for age)) income $200,000 5%))
         (def law/limit (phase-out $1,000 $210,000 $200,000 5%))",
    );
    assert!(
        p.graph.is_folded(p.id("law/limit").unwrap()),
        "calls with law fold"
    );

    let mut c = p.case();
    let kids = p.id("kids").unwrap();
    let age = p.id("kids/*/age").unwrap();
    let (a, b) = (
        c.add_member(kids, "a").unwrap(),
        c.add_member(kids, "b").unwrap(),
    );
    p.set(&mut c, (age, a), "5").unwrap();
    p.set(&mut c, (age, b), "17").unwrap();
    p.set(&mut c, p.id("income").unwrap(), "$220,000").unwrap();
    assert_eq!(get(&p, &mut c, "credit"), "$1,500.00");
    assert_eq!(get(&p, &mut c, "law/limit"), "$500.00");

    p.set(&mut c, p.id("income").unwrap(), "$300,000").unwrap();
    assert_eq!(get(&p, &mut c, "credit"), "$0.00");
    c.check_invariants();
}

#[test]
fn function_diagnostics() {
    assert_eq!(
        errors("(defn f [x : usd] (+ x 1))"),
        ["1:19: `+` can't combine usd and a plain number"],
        "checked once, even uncalled"
    );
    assert_eq!(
        errors("(defn f [x : usd] x)\n(def a (f 1))"),
        ["2:11: `f` expects usd for `x`, found number `1`"]
    );
    assert_eq!(
        errors("(defn f [x : usd] x)\n(def a (f))"),
        ["2:8: `f` takes 1 argument"]
    );
    assert_eq!(
        errors("(defn f [x : number] (g x))\n(defn g [x : number] (f x))"),
        ["2:22: recursive call to `f`"]
    );
    assert_eq!(
        errors("(defn max [x : number] x)"),
        ["1:7: `max` is already a function"]
    );
    assert_eq!(
        errors("(defn f [x : dollars] x)"),
        ["1:14: unknown type `dollars`"]
    );
    assert_eq!(
        errors("(collection c (input v : usd))\n(defn f [] v)"),
        ["2:12: `v` is a field of `c`; read it inside (sum c …) or a rule of `c`"],
        "bodies see only their parameters and globals"
    );
}

#[test]
fn input_defaults() {
    let p = program(
        "(enum status single joint)
         (input wages : usd)
         (input interest : usd :default $0)
         (input status : status :default 'single)
         (input blind : bool :default false)
         (collection w2s
           (input box1 : usd)
           (input box12 : usd :default $0))
         (def income (+ wages interest (sum w2s (+ box1 box12))))
         (def extra (if blind $2,000 $0))
         (def joint? (= status 'joint))",
    );
    let mut c = p.case();
    let (wages, interest) = (p.id("wages").unwrap(), p.id("interest").unwrap());
    let w2s = p.id("w2s").unwrap();
    assert!(p.has_default(interest) && !p.has_default(wages));

    // Defaults read until answered, and aren't asked.
    assert_eq!(get(&p, &mut c, "extra"), "$0.00");
    assert_eq!(get(&p, &mut c, "joint?"), "false");
    let income = p.id("income").unwrap();
    assert_eq!(p.unanswered(&mut c, income), vec![wages, w2s]);
    assert_eq!(
        c.unanswered(income).len(),
        3,
        "the engine still sees interest as unset"
    );
    assert!(!c.is_set(interest));

    p.set(&mut c, wages, "$50,000").unwrap();
    let acme = c.add_member(w2s, "acme").unwrap();
    let names: Vec<String> = p
        .unanswered(&mut c, income)
        .into_iter()
        .map(|f| c.name(f))
        .collect();
    assert_eq!(names, ["w2s/#acme/box1"]);
    p.set(&mut c, (p.id("w2s/*/box1").unwrap(), acme), "$1,000")
        .unwrap();
    assert_eq!(get(&p, &mut c, "income"), "$51,000.00");

    // An answer overrides the default; withdrawing it restores the default.
    p.set(&mut c, interest, "$25").unwrap();
    assert_eq!(get(&p, &mut c, "income"), "$51,025.00");
    c.unset(interest).unwrap();
    assert_eq!(get(&p, &mut c, "income"), "$51,000.00");
    c.check_invariants();
}

#[test]
fn default_diagnostics() {
    assert_eq!(
        errors("(input a : usd :default 5)"),
        ["1:25: `a` is usd, so its default can't be number `5`"]
    );
    assert_eq!(
        errors("(input b : usd)\n(input a : usd :default b)"),
        ["2:25: expected a literal, found `b`"]
    );
    assert_eq!(
        errors("(input a : usd :default)"),
        ["1:1: expected (input name : type [:default value] [:key value…])"]
    );
}
