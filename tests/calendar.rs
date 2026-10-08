//! Dates and fixed-length lists (monthly lines).

use factlet::Case;
use factlet::lisp::date::Date;
use factlet::lisp::parser::{SExprKind, read};
use factlet::lisp::{Domain, Program, Value, load};

const USD: &str = "(unit usd :prefix \"$\" :places 2)\n";

fn program(src: &str) -> Program {
    load(&format!("{USD}{src}"), &Domain::new()).unwrap_or_else(|errs| {
        let errs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        panic!("{}", errs.join("\n"))
    })
}

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

fn date(s: &str) -> Date {
    Date::parse(s).unwrap().unwrap()
}

#[test]
fn dates_parse_and_compare() {
    match &read("2024-02-29").unwrap()[0].kind {
        SExprKind::Date(d) => assert_eq!(d.to_string(), "2024-02-29"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        read("(x 2025-02-29)").unwrap_err().to_string(),
        "1:4: `2025-02-29` is not a real date"
    );
    assert_eq!(
        read("2025-13-01").unwrap_err().to_string(),
        "1:1: `2025-13-01` is not a real date"
    );
    assert!(matches!(
        read("2025-1-01").unwrap()[0].kind,
        SExprKind::Number(_)
    ));
    assert!(date("1960-12-31") < date("1961-01-01"));
}

#[test]
fn date_arithmetic() {
    assert_eq!(
        date("2025-01-31").months_until(date("2025-02-28")),
        1,
        "end of month"
    );
    assert_eq!(date("2025-01-15").months_until(date("2025-02-14")), 0);
    assert_eq!(date("2025-03-01").months_until(date("2025-01-01")), -2);
    assert_eq!(date("1961-01-01").age_on(date("2025-12-31")), 64);
    assert_eq!(date("1961-01-01").age_on(date("2026-01-01")), 65);
    assert_eq!(
        date("2008-02-29").age_on(date("2025-02-28")),
        17,
        "leap-day birthday"
    );
    assert_eq!(date("1970-01-01").days(), 0);
    assert_eq!(date("2000-03-01").days() - date("2000-02-28").days(), 2);
}

#[test]
fn dates_in_programs() {
    let p = program(
        "(input dob : date)
         (input moved-in : date)
         (def law/year-end 2025-12-31)
         (def age (age-on dob law/year-end))
         ; Tax rule: you're 65 the day before your 65th birthday.
         (def over-65 (< dob 1961-01-02))
         (def months-lived (min 12 (+ 1 (months-between moved-in law/year-end))))
         (def birth-year (year dob))
         (def days-old (days-between dob law/year-end))
         (collection kids (input dob : date))
         (def ctc-kids (count kids (< (age-on dob law/year-end) 17)))",
    );
    let mut c = p.case();
    p.set(&mut c, p.id("dob").unwrap(), "1961-01-01").unwrap();
    p.set(&mut c, p.id("moved-in").unwrap(), "2025-07-15")
        .unwrap();
    assert_eq!(get(&p, &mut c, "age"), "64");
    assert_eq!(get(&p, &mut c, "over-65"), "true");
    assert_eq!(get(&p, &mut c, "months-lived"), "6");
    assert_eq!(get(&p, &mut c, "birth-year"), "1961");
    assert_eq!(get(&p, &mut c, "days-old"), "23740");

    let kids = p.id("kids").unwrap();
    let kid_dob = p.id("kids/*/dob").unwrap();
    for (name, dob) in [
        ("a", "2009-01-01"),
        ("b", "2008-12-31"),
        ("c", "2020-06-01"),
    ] {
        let m = c.add_member(kids, name).unwrap();
        p.set(&mut c, (kid_dob, m), dob).unwrap();
    }
    assert_eq!(
        get(&p, &mut c, "ctc-kids"),
        "2",
        "b turned 17 on 2025-12-31"
    );

    assert_eq!(
        p.set(&mut c, p.id("dob").unwrap(), "$5"),
        Err("dob is date, not usd `$5`".into())
    );
}

#[test]
fn monthly_lists() {
    let p = program(
        "(input premiums : [usd 12])
         (input benchmark : [usd 12])
         (input month : number)
         (defn monthly-ptc [premium : usd slcsp : usd]
           (min premium (max $0 (- slcsp $200))))
         (def ptc/monthly (map-list monthly-ptc premiums benchmark))
         (def ptc/total (sum-list ptc/monthly))
         (def january (nth premiums 0))
         (def chosen (nth premiums (- month 1)))
         (def law/schedule [10% $100 12%])
         (def law/top-rate (nth law/schedule 2))",
    );
    let mut c = p.case();
    assert_eq!(get(&p, &mut c, "ptc/total"), "?");
    assert_eq!(get(&p, &mut c, "law/top-rate"), "0.12");

    let twelve = |s: &str| format!("[{}]", [s; 12].join(" "));
    p.set(&mut c, p.id("premiums").unwrap(), &twelve("$500"))
        .unwrap();
    let mut slcsp = vec!["$600"; 6];
    slcsp.extend(vec!["$800"; 6]);
    p.set(
        &mut c,
        p.id("benchmark").unwrap(),
        &format!("[{}]", slcsp.join(" ")),
    )
    .unwrap();
    // Six months of $400, six capped at the $500 premium.
    assert_eq!(get(&p, &mut c, "ptc/total"), "$5,400.00");
    assert_eq!(get(&p, &mut c, "january"), "$500.00");

    p.set(&mut c, p.id("month").unwrap(), "3").unwrap();
    assert_eq!(get(&p, &mut c, "chosen"), "$500.00");
    p.set(&mut c, p.id("month").unwrap(), "13").unwrap();
    assert_eq!(
        get(&p, &mut c, "chosen"),
        "<error: index 12 is out of range>"
    );

    assert_eq!(
        p.set(&mut c, p.id("premiums").unwrap(), "[$1 $2]"),
        Err("premiums is [usd 12], not [usd 2] `[$1 $2]`".into())
    );
}

#[test]
fn list_diagnostics() {
    assert_eq!(
        errors("(input xs : [usd 12])\n(def x (nth xs 12))"),
        ["2:16: 12 is out of range for [usd 12]"]
    );
    assert_eq!(
        errors("(input k : number)\n(def x (nth [1 $2] k))"),
        ["2:20: a computed index needs a list of one type, not [number usd]"]
    );
    assert_eq!(
        errors("(input xs : [usd 3])\n(def x (map-list + xs xs))"),
        ["2:18: `+` is not a defn"]
    );
    assert_eq!(
        errors(
            "(defn f [a : usd b : usd] a)\n(input xs : [usd 3])\n(input ys : [usd 4])\n(def x (map-list f xs ys))"
        ),
        ["4:23: lists of different lengths: 3 and 4"]
    );
    assert_eq!(
        errors("(defn f [a : usd] a)\n(input xs : [number 3])\n(def x (map-list f xs))"),
        ["3:20: `f` expects usd for `a`, so its list must be [usd n], not [number 3]"]
    );
    assert_eq!(
        errors("(input xs : [date 3])\n(def x (sum-list xs))"),
        ["2:18: `sum-list` needs a list of numbers, found [date 3]"]
    );
    assert_eq!(
        errors("(input xs : [usd 0])"),
        ["1:18: expected a list length, found `0`"]
    );
    assert_eq!(
        errors("(input d : date)\n(def x (+ d 1))"),
        ["2:11: `+` needs numbers, found date `d`"]
    );
    assert_eq!(
        errors("(input d : date)\n(def x (< d 5))"),
        ["2:8: `<` compares date with number"]
    );
    assert_eq!(
        errors("(def x (age-on 2025-01-01 5))"),
        ["1:27: `age-on` needs dates, found number `5`"]
    );
}
