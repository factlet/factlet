use factlet::{
    Case, Graph,
    case::{Error, Stats},
    graph::{BuildError, Kind},
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
    assert_eq!(c.stats().changed, 2);

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
            changed: 1,
            ..Default::default()
        },
        "the chain is never walked"
    );

    // Marking stops at facts already changed.
    c.set(a, 6).unwrap();
    c.set(a, 7).unwrap();
    assert_eq!(c.stats().changed, 1);
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
    assert_eq!(c.stats().changed, 2);
    assert_eq!(c.get(picked), 6);
    c.check_invariants();
}

#[test]
fn unanswered_follows_branches_taken() {
    let mut b = Graph::<i64>::builder();
    let flag = b.input("flag", 0);
    let x = b.input("x", 1);
    let y = b.input("y", 2);
    let out = b.derived("out", move |cx| {
        if cx.get(flag) != 0 {
            cx.get(x)
        } else {
            cx.get(y)
        }
    });
    let mut c = Case::new(b.build().unwrap());
    assert_eq!(&*c.deps(out), &[flag, y]);
    assert_eq!(c.unanswered(out), vec![flag, y]);

    c.set(flag, 1).unwrap();
    assert!(c.is_set(flag) && !c.is_set(x));
    assert_eq!(c.unanswered(out), vec![x]);

    c.set(x, 5).unwrap();
    assert!(c.unanswered(out).is_empty());
    c.check_invariants();
}

#[test]
fn fork_keeps_memos() {
    let mut b = Graph::<i64>::builder();
    let a = b.input("a", 1);
    let bb = b.input("b", 2);
    let fa = b.derived("fa", move |cx| cx.get(a) * 10);
    let fb = b.derived("fb", move |cx| cx.get(bb) * 10);
    let total = b.derived("total", move |cx| cx.get(fa) + cx.get(fb));
    let mut c = Case::new(b.build().unwrap());
    assert_eq!(c.get(total), 30);

    let mut what_if = c.fork();
    what_if.reset_stats();
    what_if.set(a, 5).unwrap();
    assert_eq!(what_if.get(total), 70);
    assert_eq!(
        what_if.stats().executed,
        2,
        "fa and total rerun, fb is reused"
    );
    what_if.check_invariants();

    c.reset_stats();
    assert_eq!(c.get(total), 30);
    assert_eq!(c.stats(), Stats::default(), "the original is untouched");
}

#[test]
fn law_only_rules_fold() {
    let mut b = Graph::<i64>::builder();
    let base = b.constant("base", 15_000);
    let bonus = b.constant("bonus", 750);
    let std = b.derived("std", move |cx| cx.get(base) + cx.get(bonus));
    let half = b.derived("half", move |cx| cx.get(std) / 2);
    let wages = b.input("wages", 0);
    let taxable = b.derived("taxable", move |cx| (cx.get(wages) - cx.get(std)).max(0));
    let g = b.build().unwrap();
    assert_eq!(g.folded(), 2);
    assert!(g.is_folded(std) && g.is_folded(half) && !g.is_folded(taxable));
    assert_eq!(g.kind(std), Kind::Derived);

    let mut c = Case::new(g);
    assert_eq!(c.get(half), 7_875);
    assert_eq!(&*c.deps(std), &[base, bonus]);
    assert_eq!(c.stats().executed, 0, "folded facts never run per case");

    c.set(wages, 20_000).unwrap();
    assert_eq!(c.get(taxable), 4_250);
    assert_eq!(c.stats().executed, 1);
    assert_eq!(c.set(std, 0), Err(Error("std".into())));
    c.check_invariants();
}
