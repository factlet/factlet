//! The draft's TY2025 scenarios, run against `examples/form1040/form1040.lisp`.

#[path = "../examples/form1040/tax.rs"]
mod tax;

use factlet::Case;
use factlet::case::Error;
use factlet::lisp::{Program, Value, load};

fn program() -> Program {
    load(
        include_str!("../examples/form1040/form1040.lisp"),
        &tax::domain(),
    )
    .unwrap_or_else(|errs| {
        let errs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        panic!("{}", errs.join("\n"))
    })
}

fn id(p: &Program, name: &str) -> usize {
    p.id(name).unwrap_or_else(|| panic!("no fact {name}"))
}

fn get(p: &Program, c: &mut Case<Value>, name: &str) -> String {
    c.get(id(p, name)).to_string()
}

/// A case with filing status, interest, and the given (employer, wages, withheld) W-2s.
fn case(p: &Program, status: &str, interest: &str, w2s: &[(&str, &str, &str)]) -> Case<Value> {
    let mut c = p.case();
    p.set(&mut c, id(p, "filing-status"), status).unwrap();
    p.set(&mut c, id(p, "taxable-interest"), interest).unwrap();
    c.set_empty(id(p, "w2s")).unwrap();
    for &(employer, wages, withheld) in w2s {
        let m = c.add_member(id(p, "w2s"), employer).unwrap();
        p.set(&mut c, (id(p, "w2s/*/box1-wages"), m), wages)
            .unwrap();
        p.set(&mut c, (id(p, "w2s/*/box2-withheld"), m), withheld)
            .unwrap();
    }
    c
}

#[test]
fn single_filer_one_w2() {
    let p = program();
    let mut c = case(&p, "single", "$0", &[("acme", "$58,000", "$6,000")]);
    assert_eq!(get(&p, &mut c, "line15/taxable-income"), "$42,250.00");
    // $1,192.50 + 12% x ($42,250 - $11,925) = $4,831.50
    assert_eq!(get(&p, &mut c, "line16/tax"), "$4,832.00");
    assert_eq!(get(&p, &mut c, "line34/refund"), "$1,168.00");
    assert_eq!(get(&p, &mut c, "line37/amount-owed"), "$0.00");
}

#[test]
fn joint_filers_two_w2s() {
    let p = program();
    let w2s = [
        ("acme", "$70,000", "$7,000"),
        ("globex", "$50,000", "$5,000"),
    ];
    let mut c = case(&p, "married-joint", "$500", &w2s);
    assert_eq!(get(&p, &mut c, "line11/agi"), "$120,500.00");
    assert_eq!(get(&p, &mut c, "line15/taxable-income"), "$89,000.00");
    // $2,385 + 12% x ($89,000 - $23,850)
    assert_eq!(get(&p, &mut c, "line16/tax"), "$10,203.00");
    assert_eq!(get(&p, &mut c, "line34/refund"), "$1,797.00");
}

#[test]
fn top_bracket_matches_rev_proc() {
    // Rev. Proc. 2024-40: single over $626,350 is $188,769.75 plus 37% of
    // the excess: $188,769.75 + $27,250.50 = $216,020.25.
    let p = program();
    let mut c = case(&p, "single", "$0", &[("acme", "$715,750", "$0")]);
    assert_eq!(get(&p, &mut c, "line15/taxable-income"), "$700,000.00");
    assert_eq!(get(&p, &mut c, "line16/tax"), "$216,020.00");

    let mut c = case(&p, "head-of-household", "$0", &[]);
    assert_eq!(get(&p, &mut c, "line16/tax"), "$0.00");
}

#[test]
fn law_is_folded() {
    let p = program();
    for name in ["law/std-deduction/single", "law/brackets/joint"] {
        assert!(p.graph.is_folded(id(&p, name)), "{name}");
    }
    assert_eq!(p.graph.folded(), 7);
}

#[test]
fn unanswered_questions_are_reported_where_reached() {
    let p = program();
    let mut c = p.case();
    let refund = id(&p, "line34/refund");
    assert_eq!(get(&p, &mut c, "line34/refund"), "?");
    let names = |c: &mut Case<Value>| -> Vec<String> {
        c.unanswered(refund)
            .into_iter()
            .map(|fact| c.name(fact))
            .collect()
    };
    assert_eq!(names(&mut c), ["filing-status", "w2s", "taxable-interest"]);

    p.set(&mut c, id(&p, "filing-status"), "single").unwrap();
    c.add_member(id(&p, "w2s"), "acme").unwrap();
    assert_eq!(
        names(&mut c),
        [
            "w2s/#acme/box1-wages",
            "w2s/#acme/box2-withheld",
            "taxable-interest"
        ]
    );

    c.set_empty(id(&p, "w2s")).unwrap();
    p.set(&mut c, id(&p, "taxable-interest"), "$0").unwrap();
    assert!(names(&mut c).is_empty());
    assert_eq!(get(&p, &mut c, "line34/refund"), "$0.00");
}

#[test]
fn small_changes_recompute_little() {
    let p = program();
    let mut c = case(
        &p,
        "head-of-household",
        "$0",
        &[("acme", "$20,000", "$900")],
    );
    assert_eq!(get(&p, &mut c, "line34/refund"), "$900.00");
    assert_eq!(get(&p, &mut c, "line37/amount-owed"), "$0.00");

    // Interest moves AGI but taxable income stays clamped at zero, so the tax
    // and everything below it is reused.
    c.reset_stats();
    p.set(&mut c, id(&p, "taxable-interest"), "$100").unwrap();
    assert_eq!(get(&p, &mut c, "line34/refund"), "$900.00");
    assert_eq!(get(&p, &mut c, "line37/amount-owed"), "$0.00");
    let s = c.stats();
    assert_eq!(s.executed, 2, "only AGI and taxable income rerun: {s:?}");
    assert_eq!(s.backdated, 1);

    // A withholding change never touches the tax computation.
    c.reset_stats();
    let acme = c.members(id(&p, "w2s")).unwrap().unwrap()[0];
    p.set(&mut c, (id(&p, "w2s/*/box2-withheld"), acme), "$1,000")
        .unwrap();
    assert_eq!(get(&p, &mut c, "line34/refund"), "$1,000.00");
    assert_eq!(c.stats().executed, 2, "withholding and refund");
}

#[test]
fn law_is_fixed_and_what_ifs_fork_the_case() {
    let p = program();
    let mut c = case(&p, "single", "$0", &[("acme", "$58,000", "$6,000")]);
    assert_eq!(get(&p, &mut c, "line16/tax"), "$4,832.00");
    let law = id(&p, "law/std-deduction/single");
    assert_eq!(
        c.set(law, p.value("$15,000").unwrap()),
        Err(Error::NotAnInput("law/std-deduction/single".into()))
    );

    let mut hoh = c.fork();
    p.set(&mut hoh, id(&p, "filing-status"), "head-of-household")
        .unwrap();
    // $1,700 + 12% x ($34,375 - $17,000) = $3,785
    assert_eq!(get(&p, &mut hoh, "line16/tax"), "$3,785.00");
    assert_eq!(get(&p, &mut c, "line16/tax"), "$4,832.00");
}

#[test]
fn bad_schedules_are_rejected_at_load() {
    let errs = load(
        "(def law/s [10% $100 12%])\n(def t (brackets $5 [10% 12%]))",
        &tax::domain(),
    )
    .err()
    .unwrap();
    let errs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
    assert_eq!(errs, ["2:8: `brackets`: the schedule must end with a rate"]);
}
