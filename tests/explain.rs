use factlet::{Case, Graph};

#[test]
fn explain_renders_tree() {
    let mut b = Graph::<i64>::builder();
    let base = b.constant("base", 15_000);
    let bonus = b.constant("bonus", 750);
    let std = b.derived("std", move |cx| cx.get(base) + cx.get(bonus));
    let wages = b.input("wages", 0);
    let taxable = b.derived("taxable", move |cx| (cx.get(wages) - cx.get(std)).max(0));
    let mut c = Case::new(b.build().unwrap());

    assert_eq!(
        c.explain(taxable).to_string(),
        "\
taxable = 0
├── wages = 0  (unanswered)
└── std = 15750
    ├── base = 15000
    └── bonus = 750
"
    );

    c.set(wages, 20_000).unwrap();
    let e = c.explain(taxable);
    assert_eq!(e.root.value, Some(4_250));
    assert!(e.root.children[0].set);
}

#[test]
fn repeated_facts_expand_once() {
    let mut b = Graph::<i64>::builder();
    let x = b.input("x", 1);
    let x2 = b.derived("x2", move |cx| cx.get(x) * 2);
    let y = b.derived("y", move |cx| cx.get(x2) + 1);
    let d = b.derived("d", move |cx| cx.get(x2) + cx.get(y));
    let mut c = Case::new(b.build().unwrap());
    c.set(x, 1).unwrap();

    assert_eq!(
        c.explain(d).to_string(),
        "\
d = 5
├── x2 = 2
│   └── x = 1
└── y = 3
    └── x2 = 2  (see above)
"
    );
}
