use factlet::{
    Case, Graph,
    case::{Error, Stats},
    graph::BuildError,
};

fn w2s() -> (std::sync::Arc<Graph<i64>>, usize, usize, usize, usize) {
    let mut b = Graph::<i64>::builder();
    let w2s = b.collection("w2s");
    let box1 = b.field_input(w2s, "box1Wages", 0);
    let taxed = b.field_derived(w2s, "taxed", move |cx| cx.get(box1) / 10);
    let total = b.derived("total", move |cx| {
        let members = cx.members(w2s);
        members.iter().map(|&m| cx.get((taxed, m))).sum()
    });
    (b.build().unwrap(), w2s, box1, taxed, total)
}

#[test]
fn aggregate_over_members() {
    let (g, w2s, box1, taxed, total) = w2s();
    let mut c = Case::new(g);
    assert!(!c.is_set(w2s));
    assert_eq!(c.get(total), 0);
    assert_eq!(c.unanswered(total), vec![w2s]);

    let acme = c.add_member(w2s, "acme").unwrap();
    let globex = c.add_member(w2s, "globex").unwrap();
    c.set((box1, acme), 58_000).unwrap();
    c.set((box1, globex), 12_000).unwrap();
    assert_eq!(c.get(total), 7_000);
    assert_eq!(c.get((taxed, globex)), 1_200);
    assert_eq!(
        c.members(w2s).unwrap().as_deref(),
        Some(&[acme, globex][..])
    );
    assert_eq!(c.name((box1, acme)), "w2s/#acme/box1Wages");
    assert_eq!(c.graph().name(box1), "w2s/*/box1Wages");
    c.check_invariants();
}

#[test]
fn member_edit_dirties_only_its_row() {
    let (g, w2s, box1, _, total) = w2s();
    let mut c = Case::new(g);
    let acme = c.add_member(w2s, "acme").unwrap();
    let globex = c.add_member(w2s, "globex").unwrap();
    c.set((box1, acme), 10_000).unwrap();
    c.set((box1, globex), 20_000).unwrap();
    assert_eq!(c.get(total), 3_000);

    c.reset_stats();
    c.set((box1, acme), 30_000).unwrap();
    assert_eq!(c.get(total), 5_000);
    assert_eq!(
        c.stats(),
        Stats {
            executed: 2,
            changed: 2,
            ..Default::default()
        },
        "acme's taxed and total rerun; globex's row isn't touched"
    );
    c.check_invariants();
}

#[test]
fn add_member_dirties_aggregate() {
    let (g, w2s, box1, _, total) = w2s();
    let mut c = Case::new(g);
    let acme = c.add_member(w2s, "acme").unwrap();
    c.set((box1, acme), 10_000).unwrap();
    assert_eq!(c.get(total), 1_000);

    c.reset_stats();
    let globex = c.add_member(w2s, "globex").unwrap();
    assert_eq!(c.get(total), 1_000, "globex's box 1 is still 0");
    assert_eq!(c.stats().executed, 2, "total and globex's taxed");
    assert_eq!(c.stats().backdated, 1);

    c.set((box1, globex), 5_000).unwrap();
    assert_eq!(c.get(total), 1_500);
    c.check_invariants();
}

#[test]
fn scope_errors() {
    let (g, w2s, box1, taxed, total) = w2s();
    let mut c = Case::new(g);
    let acme = c.add_member(w2s, "acme").unwrap();
    assert_eq!(
        c.add_member(w2s, "acme"),
        Err(Error::DuplicateMember {
            collection: "w2s".into(),
            name: "acme".into()
        })
    );
    assert_eq!(
        c.set(box1, 1),
        Err(Error::WrongScope("w2s/*/box1Wages".into()))
    );
    assert_eq!(
        c.set((box1, acme), 1).and(c.set((taxed, acme), 1)),
        Err(Error::NotAnInput("w2s/#acme/taxed".into()))
    );
    assert_eq!(
        c.add_member(total, "x"),
        Err(Error::NotACollection("total".into()))
    );

    let mut b = Graph::<i64>::builder();
    let x = b.input("x", 0);
    b.field_input(x, "y", 0);
    assert_eq!(
        b.build().err(),
        Some(vec![BuildError::NotACollection {
            field: "x/*/y".into(),
            scope: "x".into()
        }])
    );
}

#[test]
fn remove_member_updates_aggregate() {
    let (g, w2s, box1, _, total) = w2s();
    let mut c = Case::new(g);
    let acme = c.add_member(w2s, "acme").unwrap();
    let globex = c.add_member(w2s, "globex").unwrap();
    c.set((box1, acme), 10_000).unwrap();
    c.set((box1, globex), 20_000).unwrap();
    assert_eq!(c.get(total), 3_000);

    c.remove_member(w2s, acme).unwrap();
    c.check_invariants();
    assert_eq!(c.get(total), 2_000);
    assert_eq!(c.members(w2s).unwrap().as_deref(), Some(&[globex][..]));
    assert_eq!(c.name((box1, acme)), "w2s/#0(removed)/box1Wages");
    assert_eq!(
        c.set((box1, acme), 1),
        Err(Error::NoSuchMember("w2s/#0(removed)/box1Wages".into()))
    );
    assert_eq!(
        c.remove_member(w2s, acme),
        Err(Error::NoSuchMember("w2s/#0".into()))
    );

    let acme2 = c.add_member(w2s, "acme").unwrap();
    assert_ne!(acme2, acme, "member ids are never reused");
    c.set((box1, acme2), 5_000).unwrap();
    assert_eq!(c.get(total), 2_500);
    c.check_invariants();
}

#[test]
fn removed_rows_are_unhooked() {
    let mut b = Graph::<i64>::builder();
    let rate = b.input("rate", 10);
    let w2s = b.collection("w2s");
    let box1 = b.field_input(w2s, "box1Wages", 0);
    let taxed = b.field_derived(w2s, "taxed", move |cx| cx.get(box1) * cx.get(rate) / 100);
    let total = b.derived("total", move |cx| {
        let members = cx.members(w2s);
        members.iter().map(|&m| cx.get((taxed, m))).sum()
    });
    let mut c = Case::new(b.build().unwrap());
    let acme = c.add_member(w2s, "acme").unwrap();
    let globex = c.add_member(w2s, "globex").unwrap();
    c.set((box1, acme), 10_000).unwrap();
    c.set((box1, globex), 20_000).unwrap();
    assert_eq!(c.get(total), 3_000);

    c.remove_member(w2s, acme).unwrap();
    assert_eq!(c.get(total), 2_000);
    c.reset_stats();
    c.set(rate, 20).unwrap();
    assert_eq!(c.stats().changed, 2, "globex's taxed and total, not acme's");
    assert_eq!(c.get(total), 4_000);
    c.check_invariants();
}

#[test]
fn set_empty_answers_collection() {
    let (g, w2s, box1, _, total) = w2s();
    let mut c = Case::new(g);
    assert_eq!(c.unanswered(total), vec![w2s]);
    c.set_empty(w2s).unwrap();
    assert!(c.is_set(w2s));
    assert!(c.unanswered(total).is_empty());
    assert_eq!(c.get(total), 0);

    let acme = c.add_member(w2s, "acme").unwrap();
    c.set((box1, acme), 10_000).unwrap();
    assert_eq!(c.get(total), 1_000);
    c.set_empty(w2s).unwrap();
    assert_eq!(c.members(w2s).unwrap().as_deref(), Some(&[][..]));
    assert_eq!(c.get(total), 0);
    c.check_invariants();

    c.reset_stats();
    c.set_empty(w2s).unwrap();
    assert_eq!(c.stats(), Stats::default(), "already empty is a no-op");
}
