use factlet::lisp::{Domain, Program, load, load_files};

const USD: &str = "(unit usd :prefix \"$\" :places 2)\n";

fn files<'a>(files: &'a [(&'a str, &'a str)]) -> impl FnMut(&str) -> Option<String> + 'a {
    |name| files.iter().find(|f| f.0 == name).map(|f| f.1.to_string())
}

fn program(src: &str) -> Program {
    load(&format!("{USD}{src}"), &Domain::new()).unwrap_or_else(|errs| {
        let errs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        panic!("{}", errs.join("\n"))
    })
}

fn errors(main: &str, all: &[(&str, &str)]) -> Vec<String> {
    match load_files(main, files(all), &Domain::new()) {
        Ok(_) => panic!("loaded"),
        Err(errs) => errs.iter().map(|e| e.to_string()).collect(),
    }
}

#[test]
fn modules_prefix_names() {
    let p = program(
        "(input agi : usd)
         (module sch-a
           (input medical : usd)
           (def line4 (excess medical (* 7.5% agi)))   ; agi is top-level
           (module limits (def floor $100))
           (def line17 (+ line4 limits/floor)))
         (module sch-b
           (input interest : usd :default $0)
           (def line4 interest))                        ; no clash with sch-a/line4
         (defn excess [a : usd b : usd] (max $0 (- a b)))
         (def itemized (+ sch-a/line17 sch-b/line4))",
    );
    for name in [
        "sch-a/line4",
        "sch-a/limits/floor",
        "sch-b/line4",
        "itemized",
    ] {
        assert!(p.id(name).is_some(), "{name}");
    }
    assert!(p.id("line4").is_none());

    let mut c = p.case();
    p.set(&mut c, p.id("agi").unwrap(), "$40,000").unwrap();
    p.set(&mut c, p.id("sch-a/medical").unwrap(), "$5,000")
        .unwrap();
    assert_eq!(
        p.get(&mut c, "sch-a/line17").unwrap().to_string(),
        "$2,100.00"
    );
    assert_eq!(p.get(&mut c, "itemized").unwrap().to_string(), "$2,100.00");
}

#[test]
fn inner_names_shadow_outer_ones() {
    let p = program(
        "(def rate 10%)
         (module state
           (def rate 5%)
           (defn tax [x : usd] (* x rate))
           (def mine (tax $100)))
         (def federal (* $100 rate))
         (def theirs (state/tax $1,000))",
    );
    let mut c = p.case();
    assert_eq!(p.get(&mut c, "state/mine").unwrap().to_string(), "$5.00");
    assert_eq!(p.get(&mut c, "federal").unwrap().to_string(), "$10.00");
    assert_eq!(
        p.get(&mut c, "theirs").unwrap().to_string(),
        "$50.00",
        "a module's function reads its own module's names"
    );
}

#[test]
fn collections_and_fixpoints_in_modules() {
    let p = program(
        "(module w2
           (collection forms (input wages : usd))
           (def total (sum forms wages)))
         (module loop
           (fixpoint x :start 0 :within 0.001)
           (def x (/ (+ x 2) 2)))",
    );
    assert!(p.id("w2/forms/*/wages").is_some());
    let mut c = p.case();
    let forms = p.id("w2/forms").unwrap();
    let m = c.add_member(forms, "acme").unwrap();
    p.set(&mut c, (p.id("w2/forms/*/wages").unwrap(), m), "$7")
        .unwrap();
    assert_eq!(p.get(&mut c, "w2/total").unwrap().to_string(), "$7.00");
    assert!(p.id("fixpoint/loop/x").is_some());
    let x = p.get(&mut c, "loop/x").unwrap().to_string();
    assert!(x.starts_with("1.99") || x == "2", "{x}");
}

#[test]
fn files_include_each_other() {
    let all = [
        (
            "form1040.lisp",
            "(include \"units.lisp\")
             (include \"sch-a.lisp\")
             (include \"units.lisp\")          ; again: no effect
             (input agi : usd)
             (module law (include \"law-2025.lisp\"))
             (def deduction (max law/std-deduction sch-a/line17))",
        ),
        ("units.lisp", "(unit usd :prefix \"$\" :places 2)"),
        (
            "sch-a.lisp",
            "(module sch-a
               (input medical : usd)
               (def line17 (max $0 (- medical (* 7.5% agi)))))",
        ),
        ("law-2025.lisp", "(def std-deduction $15,750)"),
    ];
    let p = load_files("form1040.lisp", files(&all), &Domain::new()).unwrap();
    assert!(p.graph.is_folded(p.id("law/std-deduction").unwrap()));
    let mut c = p.case();
    p.set(&mut c, p.id("agi").unwrap(), "$40,000").unwrap();
    p.set(&mut c, p.id("sch-a/medical").unwrap(), "$20,000")
        .unwrap();
    assert_eq!(
        p.get(&mut c, "deduction").unwrap().to_string(),
        "$17,000.00"
    );
}

#[test]
fn diagnostics_name_their_file() {
    let all = [
        ("main.lisp", "(include \"a.lisp\")\n(def x (+ y 1))"),
        ("a.lisp", "(def z\n  (+ 1 $2))"),
    ];
    assert_eq!(
        errors("main.lisp", &all),
        [
            "main.lisp:2:11: unknown name `y`",
            "a.lisp:2:8: `$2`: unknown unit prefix `$`",
        ]
    );
    assert_eq!(
        errors("main.lisp", &[("main.lisp", "(include \"gone.lisp\")")]),
        ["main.lisp:1:1: can't find `gone.lisp`"]
    );
    assert_eq!(
        errors(
            "main.lisp",
            &[
                ("main.lisp", "(include \"bad.lisp\")"),
                ("bad.lisp", "(def x")
            ]
        ),
        ["bad.lisp:1:1: unclosed, expected `)`"]
    );
    assert_eq!(
        errors(
            "a.lisp",
            &[
                ("a.lisp", "(include \"b.lisp\")"),
                ("b.lisp", "(include \"a.lisp\")")
            ]
        ),
        ["b.lisp:1:1: include cycle: a.lisp -> b.lisp -> a.lisp"]
    );
    assert_eq!(
        errors("none.lisp", &[]),
        ["none.lisp: can't find `none.lisp`"]
    );
    assert_eq!(
        errors("m.lisp", &[("m.lisp", "(module (def x 1))")]),
        ["m.lisp:1:1: expected (module name forms…)"]
    );
}
