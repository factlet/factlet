use factlet::lisp::{Domain, Program, load};

const SRC: &str = r#"
(unit usd :prefix "$" :places 2)
(input wages : usd :label "Wages" :line 1a)
(input interest : usd :default $0 :label "Taxable interest" :line 2b :form "1099-INT")
(collection w2s :label "Forms W-2"
  (input box1 : usd :label "Wages, tips, other compensation" :line 1))
(def law/deduction :cite "IRC §63(c)(2)" $15,750)
(def line15/taxable
  :label "Taxable income" :line 15
  (max $0 (- (+ wages interest (sum w2s box1)) law/deduction)))
"#;

fn program() -> Program {
    load(SRC, &Domain::new()).unwrap_or_else(|errs| {
        let errs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
        panic!("{}", errs.join("\n"))
    })
}

#[test]
fn metadata_is_kept_by_id() {
    let p = program();
    let interest = p.meta(p.id("interest").unwrap()).unwrap();
    assert_eq!(interest.label(), Some("Taxable interest"));
    assert_eq!(interest.line(), Some("2b"));
    assert_eq!(interest.get("form"), Some("1099-INT"), "any key is kept");
    assert_eq!(
        interest.iter().collect::<Vec<_>>(),
        [
            ("label", "Taxable interest"),
            ("line", "2b"),
            ("form", "1099-INT")
        ]
    );
    assert!(p.has_default(p.id("interest").unwrap()));

    let line15 = p.meta(p.id("line15/taxable").unwrap()).unwrap();
    assert_eq!(
        line15.summary().as_deref(),
        Some("Taxable income (line 15)")
    );
    let law = p.meta(p.id("law/deduction").unwrap()).unwrap();
    assert_eq!(law.summary().as_deref(), Some("IRC §63(c)(2)"));
    assert_eq!(
        p.meta(p.id("w2s/*/box1").unwrap()).unwrap().line(),
        Some("1")
    );
    assert!(p.graph.is_folded(p.id("law/deduction").unwrap()));
}

#[test]
fn explain_shows_labels_lines_and_citations() {
    let p = program();
    let mut c = p.case();
    p.set(&mut c, p.id("wages").unwrap(), "$10,000").unwrap();
    let w2s = p.id("w2s").unwrap();
    let acme = c.add_member(w2s, "acme").unwrap();
    p.set(&mut c, (p.id("w2s/*/box1").unwrap(), acme), "$20,000")
        .unwrap();
    assert_eq!(
        p.explain(&mut c, "line15/taxable").unwrap().to_string(),
        "\
line15/taxable = $14,250.00  ; Taxable income (line 15)
├── wages = $10,000.00  ; Wages (line 1a)
├── interest = $0.00  (unanswered)  ; Taxable interest (line 2b)
├── w2s = [#acme]  ; Forms W-2
├── w2s/#acme/box1 = $20,000.00  ; Wages, tips, other compensation (line 1)
└── law/deduction = $15,750.00  ; IRC §63(c)(2)
"
    );
    // The plain tree is unchanged.
    let tree = &p.explain(&mut c, "line15/taxable").unwrap().tree;
    assert!(
        tree.to_string()
            .starts_with("line15/taxable = $14,250.00\n")
    );
}

#[test]
fn metadata_diagnostics() {
    let errors = |src: &str| -> Vec<String> {
        match load(src, &Domain::new()) {
            Ok(_) => panic!("loaded"),
            Err(errs) => errs.iter().map(|e| e.to_string()).collect(),
        }
    };
    assert_eq!(
        errors("(def x :label \"a\" :label \"b\" 1)"),
        ["1:19: `:label` given twice"]
    );
    assert_eq!(
        errors("(def x :label [1 2] 1)"),
        ["1:15: `:label` takes text, found `[1 2]`"]
    );
    assert_eq!(
        errors("(def x :label \"a\")"),
        ["1:1: expected (def name [:key value…] expression)"]
    );
}
