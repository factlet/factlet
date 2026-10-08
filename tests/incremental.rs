use factlet::{
    Case, Graph,
    case::{Error, Stats},
    graph::BuildError,
};

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
    assert_eq!(
        c.stats().marked_green,
        0,
        "fb isn't downstream of a, so it isn't checked"
    );
    assert_eq!(c.stats().dirtied, 2);

    c.reset_stats();
    c.set(a, 5).unwrap();
    assert_eq!(c.get(total), 70);
    assert_eq!(
        c.stats(),
        Default::default(),
        "writing the same value is a no-op"
    );
    c.check_invariants();
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
    c.check_invariants();
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

#[test]
fn unrelated_reads_are_free() {
    let mut b = Graph::<i64>::builder();
    let a = b.input("a", 0);
    let fa = b.derived("fa", move |cx| cx.get(a) + 1);
    let x = b.input("x", 0);
    let mut chain = b.derived("chain/0", move |cx| cx.get(x));
    for i in 1..1_000 {
        let prev = chain;
        chain = b.derived(&format!("chain/{i}"), move |cx| cx.get(prev) + 1);
    }
    let mut c = Case::new(b.build().unwrap());
    assert_eq!(c.get(chain), 999);
    assert_eq!(c.get(fa), 1);

    c.reset_stats();
    c.set(a, 5).unwrap();
    assert_eq!(c.get(chain), 999);
    assert_eq!(
        c.stats(),
        Stats {
            dirtied: 1,
            ..Default::default()
        },
        "the chain is never walked"
    );

    // Marking stops at facts already dirty.
    c.set(a, 6).unwrap();
    c.set(a, 7).unwrap();
    assert_eq!(c.stats().dirtied, 1);
    assert_eq!(c.get(fa), 8);
    c.check_invariants();
}

#[test]
fn branches_not_taken_are_not_dependencies() {
    let mut b = Graph::<i64>::builder();
    let flag = b.input("flag", 1);
    let x = b.input("x", 0);
    let picked = b.derived(
        "picked",
        move |cx| if cx.get(flag) != 0 { cx.get(x) } else { -1 },
    );
    let mut c = Case::new(b.build().unwrap());
    assert_eq!(c.get(picked), 0);

    c.set(flag, 0).unwrap();
    assert_eq!(c.get(picked), -1);
    c.check_invariants();

    c.reset_stats();
    c.set(x, 5).unwrap();
    assert_eq!(c.get(picked), -1);
    assert_eq!(c.stats(), Stats::default(), "x is no longer read");

    c.set(flag, 1).unwrap();
    assert_eq!(c.get(picked), 5);
    c.set(x, 6).unwrap();
    assert_eq!(c.stats().dirtied, 2);
    assert_eq!(c.get(picked), 6);
    c.check_invariants();
}
