use factlet::{Case, Graph, case::Error, graph::BuildError};

#[test]
fn only_affected_facts_rerun() {
    let mut b = Graph::<i64>::builder();
    let a = b.input("a", 1);
    let bb = b.input("b", 2);
    let fa = b.derived("fa", move |cx| cx.get(a) * 10);
    let fb = b.derived("fb", move |cx| cx.get(bb) * 10);
    let total = b.derived("total", move |cx| cx.get(fa) + cx.get(fb));
    let mut c = Case::new(b.build().unwrap());

    assert_eq!(c.get(total), 30);
    assert_eq!(c.stats().executed, 3);

    c.reset_stats();
    c.set(a, 5).unwrap();
    assert_eq!(c.get(total), 70);
    assert_eq!(c.stats().executed, 2, "fa and total rerun");
    assert_eq!(c.stats().marked_green, 1, "fb is checked, not rerun");

    c.reset_stats();
    c.set(a, 5).unwrap();
    assert_eq!(c.get(total), 70);
    assert_eq!(
        c.stats(),
        Default::default(),
        "writing the same value is a no-op"
    );
}

#[test]
fn backdating_stops_propagation() {
    let mut b = Graph::<i64>::builder();
    let x = b.input("x", 0);
    let clamped = b.derived("clamped", move |cx| cx.get(x).min(10));
    let doubled = b.derived("doubled", move |cx| cx.get(clamped) * 2);
    let mut c = Case::new(b.build().unwrap());
    c.set(x, 20).unwrap();
    assert_eq!(c.get(doubled), 20);

    c.reset_stats();
    c.set(x, 30).unwrap();
    assert_eq!(c.get(doubled), 20);
    assert_eq!(
        (
            c.stats().executed,
            c.stats().backdated,
            c.stats().marked_green
        ),
        (1, 1, 1)
    );

    c.unset(x).unwrap();
    assert_eq!(c.get(doubled), 0);
}

#[test]
fn only_inputs_are_written() {
    let mut b = Graph::<i64>::builder();
    let k = b.constant("k", 1);
    let d = b.derived("d", move |cx| cx.get(k));
    let mut c = Case::new(b.build().unwrap());
    assert_eq!(c.set(k, 2), Err(Error("k".into())));
    assert_eq!(c.set(d, 2), Err(Error("d".into())));

    let mut b = Graph::<i64>::builder();
    b.input("a", 0);
    b.input("a", 0);
    assert_eq!(
        b.build().err(),
        Some(vec![BuildError::DuplicateName("a".into())])
    );
}
