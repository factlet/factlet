use crate::graph::{Def, Fact, Graph, Kind, Member};
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

/// What a removed member's facts report: changed after everything.
const REMOVED: u64 = u64::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stats {
    pub executed: u64,
    pub marked_green: u64,
    pub backdated: u64,
    pub changed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    NotAnInput(String),
    NotACollection(String),
    NoSuchMember(String),
    /// A collection field named without a member, or a global fact with one.
    WrongScope(String),
    DuplicateMember {
        collection: String,
        name: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotAnInput(n) => write!(f, "{n} is not an input and cannot be set"),
            Error::NotACollection(n) => write!(f, "{n} is not a collection"),
            Error::NoSuchMember(n) => write!(f, "{n}: no such member"),
            Error::WrongScope(n) => {
                write!(
                    f,
                    "{n}: name a member for collection fields, and only for them"
                )
            }
            Error::DuplicateMember { collection, name } => {
                write!(f, "{collection} already has #{name}")
            }
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone)]
struct Slot<V> {
    value: Option<V>,
    changed_at: u64,
    verified_at: u64,
    deps: Arc<[Fact]>,
    dependents: Vec<Fact>,
    changed: bool,
}

impl<V> Slot<V> {
    fn empty() -> Self {
        Slot {
            value: None,
            changed_at: 0,
            verified_at: 0,
            deps: Arc::new([]),
            dependents: Vec::new(),
            changed: false,
        }
    }
}

/// A member's name and fields.
type Row<V> = (Arc<str>, Box<[Slot<V>]>);

#[derive(Clone)]
struct Collection<V> {
    /// `None` until answered.
    members: Option<Arc<[Member]>>,
    changed_at: u64,
    dependents: Vec<Fact>,
    /// Each member's name and fields, indexed by member.
    /// Indexed by member; `None` once removed.
    rows: Vec<Option<Row<V>>>,
}

#[derive(Clone)]
struct State<V> {
    slots: Box<[Slot<V>]>,
    collections: Box<[Collection<V>]>,
    revision: u64,
    stats: Stats,
    deps: Vec<Fact>,
    frames: Vec<usize>,
}

pub struct Case<V> {
    graph: Arc<Graph<V>>,
    state: State<V>,
}

impl<V: Clone> Clone for Case<V> {
    fn clone(&self) -> Self {
        Case {
            graph: self.graph.clone(),
            state: self.state.clone(),
        }
    }
}

impl<V: Clone + PartialEq> Case<V> {
    pub fn new(graph: Arc<Graph<V>>) -> Self {
        let slots = (0..graph.len()).map(|_| Slot::empty()).collect();
        let collections = graph
            .collections
            .iter()
            .map(|_| Collection {
                members: None,
                changed_at: 0,
                dependents: Vec::new(),
                rows: Vec::new(),
            })
            .collect();
        let state = State {
            slots,
            collections,
            revision: 0,
            stats: Stats::default(),
            deps: Vec::new(),
            frames: Vec::new(),
        };
        Case { graph, state }
    }

    pub fn graph(&self) -> &Arc<Graph<V>> {
        &self.graph
    }

    pub fn set(&mut self, fact: impl Into<Fact>, value: V) -> Result<(), Error> {
        self.write(fact.into(), Some(value))
    }

    pub fn unset(&mut self, fact: impl Into<Fact>) -> Result<(), Error> {
        self.write(fact.into(), None)
    }

    fn write(&mut self, fact: Fact, new: Option<V>) -> Result<(), Error> {
        let g = &*self.graph;
        if !matches!(g.defs[fact.id], Def::Input(_)) {
            return Err(Error::NotAnInput(self.name(fact)));
        }
        self.check_scope(fact)?;
        if self.state.slot(g, fact).is_none() {
            return Err(Error::NoSuchMember(self.name(fact)));
        }
        let revision = self.state.revision + 1;
        let slot = self.state.slot_mut(g, fact).expect("checked above");
        if slot.value != new {
            slot.value = new;
            slot.changed_at = revision;
            self.state.revision = revision;
            self.state.mark_dependents(g, fact);
        }
        Ok(())
    }

    pub fn add_member(&mut self, collection: usize, name: &str) -> Result<Member, Error> {
        let c = self.collection_index(collection)?;
        let g = &*self.graph;
        let coll = &mut self.state.collections[c];
        if coll.rows.iter().flatten().any(|(n, _)| &**n == name) {
            return Err(Error::DuplicateMember {
                collection: g.name(collection).to_string(),
                name: name.to_string(),
            });
        }
        let member = Member(u32::try_from(coll.rows.len()).expect("too many members"));
        let slots = g.collections[c].iter().map(|_| Slot::empty()).collect();
        coll.rows.push(Some((name.into(), slots)));
        let mut members = coll.members.as_deref().unwrap_or_default().to_vec();
        members.push(member);
        coll.members = Some(members.into());
        self.state.revision += 1;
        coll.changed_at = self.state.revision;
        self.state.mark_dependents(g, collection.into());
        Ok(member)
    }

    pub fn remove_member(&mut self, collection: usize, member: Member) -> Result<(), Error> {
        let c = self.collection_index(collection)?;
        if self.member_name(collection, member).is_none() {
            let name = self.graph.name(collection);
            return Err(Error::NoSuchMember(format!("{name}/#{}", member.0)));
        }
        let g = &*self.graph;
        self.state.drop_row(g, c, member);
        let coll = &mut self.state.collections[c];
        let members = coll.members.iter().flat_map(|m| m.iter()).copied();
        coll.members = Some(members.filter(|&m| m != member).collect());
        self.state.revision += 1;
        coll.changed_at = self.state.revision;
        self.state.mark_dependents(g, collection.into());
        Ok(())
    }

    /// Answer that a collection has no members, as opposed to not having asked.
    pub fn set_empty(&mut self, collection: usize) -> Result<(), Error> {
        let c = self.collection_index(collection)?;
        let members = self.state.collections[c].members.clone();
        if members.as_deref().is_some_and(<[Member]>::is_empty) {
            return Ok(());
        }
        let g = &*self.graph;
        for &m in members.iter().flat_map(|m| m.iter()) {
            self.state.drop_row(g, c, m);
        }
        let coll = &mut self.state.collections[c];
        coll.members = Some(Arc::new([]));
        self.state.revision += 1;
        coll.changed_at = self.state.revision;
        self.state.mark_dependents(g, collection.into());
        Ok(())
    }

    /// A collection's members, or `None` if unanswered.
    pub fn members(&self, collection: usize) -> Result<Option<Arc<[Member]>>, Error> {
        let c = self.collection_index(collection)?;
        Ok(self.state.collections[c].members.clone())
    }

    pub fn member_name(&self, collection: usize, member: Member) -> Option<&str> {
        let c = self.collection_index(collection).ok()?;
        let (name, _) = self.state.collections[c]
            .rows
            .get(member.0 as usize)?
            .as_ref()?;
        Some(name)
    }

    /// `line11/agi`, or `w2s/#acme/box1Wages` for a member's field.
    pub fn name(&self, fact: impl Into<Fact>) -> String {
        let fact = fact.into();
        let g = &self.graph;
        match (g.scopes[fact.id], fact.member) {
            (Some(c), Some(m)) => {
                let field = &g.name(fact.id)[g.name(c).len() + 3..];
                match self.member_name(c, m) {
                    Some(member) => format!("{}/#{member}/{field}", g.name(c)),
                    None => format!("{}/#{}(removed)/{field}", g.name(c), m.0),
                }
            }
            _ => g.name(fact.id).to_string(),
        }
    }

    fn collection_index(&self, id: usize) -> Result<usize, Error> {
        match self.graph.defs[id] {
            Def::Collection => Ok(self.graph.pos[id]),
            _ => Err(Error::NotACollection(self.graph.name(id).to_string())),
        }
    }

    fn check_scope(&self, fact: Fact) -> Result<(), Error> {
        if self.graph.scopes[fact.id].is_some() != fact.member.is_some() {
            return Err(Error::WrongScope(self.graph.name(fact.id).to_string()));
        }
        Ok(())
    }

    pub fn get(&mut self, fact: impl Into<Fact>) -> V {
        let fact = fact.into();
        if let Err(e) = self.check_scope(fact) {
            panic!("{e}");
        }
        self.state.refresh(&self.graph, fact);
        self.state.value(&self.graph, fact).clone()
    }

    pub fn is_set(&self, fact: impl Into<Fact>) -> bool {
        self.state.is_set(&self.graph, fact.into())
    }

    pub fn deps(&mut self, fact: impl Into<Fact>) -> Arc<[Fact]> {
        let fact = fact.into();
        match &self.graph.defs[fact.id] {
            Def::Folded { deps, .. } => deps.clone(),
            Def::Derived(_) => {
                self.get(fact);
                self.state
                    .slot(&self.graph, fact)
                    .expect("live")
                    .deps
                    .clone()
            }
            _ => Arc::new([]),
        }
    }

    pub fn unanswered(&mut self, fact: impl Into<Fact>) -> Vec<Fact> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![fact.into()];
        while let Some(f) = stack.pop() {
            if !seen.insert(f) {
                continue;
            }
            match self.graph.kind(f.id) {
                Kind::Derived => stack.extend(self.deps(f).iter().rev()),
                _ if !self.is_set(f) => out.push(f),
                _ => {}
            }
        }
        out.sort_unstable();
        out
    }

    pub fn fork(&self) -> Self {
        self.clone()
    }

    pub fn revision(&self) -> u64 {
        self.state.revision
    }

    pub fn stats(&self) -> Stats {
        self.state.stats
    }

    pub fn reset_stats(&mut self) {
        self.state.stats = Stats::default();
    }

    #[doc(hidden)]
    pub fn check_invariants(&self) {
        let (g, s) = (&*self.graph, &self.state);
        let mut facts: Vec<Fact> = (0..g.len())
            .filter(|&id| g.scopes[id].is_none())
            .map(Fact::from)
            .collect();
        for (c, fields) in g.collections.iter().enumerate() {
            for m in s.collections[c].members.iter().flat_map(|m| m.iter()) {
                facts.extend(fields.iter().map(|&f| Fact::from((f, *m))));
            }
        }

        for &f in &facts {
            let dependents = s.dependents(g, f);
            let mut unique = dependents.to_vec();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                unique.len(),
                dependents.len(),
                "{} lists a dependent twice",
                self.name(f)
            );
            for &d in dependents {
                assert!(
                    s.slot(g, d).expect("live").deps.contains(&f),
                    "{} lists {}, which doesn't read it",
                    self.name(f),
                    self.name(d)
                );
            }
            if !matches!(g.defs[f.id], Def::Derived(_)) {
                continue;
            }
            let slot = s.slot(g, f).expect("live");
            for &d in slot.deps.iter() {
                if g.is_fixed(d.id) {
                    continue;
                }
                let removed = d.member.is_some() && s.slot(g, d).is_none();
                assert!(
                    if removed {
                        slot.changed
                    } else {
                        s.dependents(g, d).contains(&f)
                    },
                    "{} reads {}, which doesn't list it",
                    self.name(f),
                    self.name(d)
                );
                if let Some(dep) = s.slot(g, d) {
                    assert!(
                        slot.changed || !dep.changed,
                        "clean {} reads changed {}",
                        self.name(f),
                        self.name(d)
                    );
                }
            }
        }
    }
}

impl<V: Clone + PartialEq> State<V> {
    /// An input's or rule's storage; `None` for anything else, or a member
    /// that doesn't exist.
    fn slot(&self, g: &Graph<V>, f: Fact) -> Option<&Slot<V>> {
        if !matches!(g.defs[f.id], Def::Input(_) | Def::Derived(_)) {
            return None;
        }
        match (g.scopes[f.id], f.member) {
            (Some(c), Some(m)) => {
                let (_, row) = self.collections[g.pos[c]]
                    .rows
                    .get(m.0 as usize)?
                    .as_ref()?;
                Some(&row[g.pos[f.id]])
            }
            _ => Some(&self.slots[f.id]),
        }
    }

    fn slot_mut(&mut self, g: &Graph<V>, f: Fact) -> Option<&mut Slot<V>> {
        if !matches!(g.defs[f.id], Def::Input(_) | Def::Derived(_)) {
            return None;
        }
        match (g.scopes[f.id], f.member) {
            (Some(c), Some(m)) => {
                let (_, row) = self.collections[g.pos[c]]
                    .rows
                    .get_mut(m.0 as usize)?
                    .as_mut()?;
                Some(&mut row[g.pos[f.id]])
            }
            _ => Some(&mut self.slots[f.id]),
        }
    }

    fn dependents(&self, g: &Graph<V>, f: Fact) -> &[Fact] {
        match g.defs[f.id] {
            Def::Collection => &self.collections[g.pos[f.id]].dependents,
            _ => self.slot(g, f).map_or(&[], |s| &s.dependents),
        }
    }

    fn dependents_mut(&mut self, g: &Graph<V>, f: Fact) -> Option<&mut Vec<Fact>> {
        match g.defs[f.id] {
            Def::Collection => Some(&mut self.collections[g.pos[f.id]].dependents),
            _ => self.slot_mut(g, f).map(|s| &mut s.dependents),
        }
    }

    fn is_set(&self, g: &Graph<V>, f: Fact) -> bool {
        match g.defs[f.id] {
            Def::Collection => self.collections[g.pos[f.id]].members.is_some(),
            Def::Input(_) => self.slot(g, f).is_some_and(|s| s.value.is_some()),
            _ => true,
        }
    }

    /// Dirty whatever read a member's facts, unhook its rules from what
    /// they read, and drop its row.
    fn drop_row(&mut self, g: &Graph<V>, c: usize, member: Member) {
        for &field in g.collections[c].iter() {
            let f = Fact::from((field, member));
            self.mark_dependents(g, f);
            let slot = self.slot_mut(g, f).expect("live");
            let deps = std::mem::replace(&mut slot.deps, Arc::new([]));
            for &d in deps.iter() {
                if let Some(list) = self.dependents_mut(g, d)
                    && let Some(p) = list.iter().position(|&x| x == f)
                {
                    list.swap_remove(p);
                }
            }
        }
        self.collections[c].rows[member.0 as usize] = None;
    }

    fn mark_dependents(&mut self, g: &Graph<V>, f: Fact) {
        let mut stack = self.dependents(g, f).to_vec();
        while let Some(d) = stack.pop() {
            let slot = self.slot_mut(g, d).expect("dependents are live rules");
            if !slot.changed {
                slot.changed = true;
                stack.extend_from_slice(&slot.dependents);
                self.stats.changed += 1;
            }
        }
    }

    /// Update reverse edges after `f`'s reads changed from `old` to `new`.
    fn relink(&mut self, g: &Graph<V>, f: Fact, old: &[Fact], new: &[Fact]) {
        let sorted = |s: &[Fact]| {
            let mut v = s.to_vec();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (old, new) = (sorted(old), sorted(new));
        for &d in old.iter().filter(|d| new.binary_search(d).is_err()) {
            if let Some(list) = self.dependents_mut(g, d)
                && let Some(p) = list.iter().position(|&x| x == f)
            {
                list.swap_remove(p);
            }
        }
        for &d in new.iter().filter(|d| old.binary_search(d).is_err()) {
            if !g.is_fixed(d.id)
                && let Some(list) = self.dependents_mut(g, d)
            {
                list.push(f);
            }
        }
    }

    fn value<'a>(&'a self, g: &'a Graph<V>, f: Fact) -> &'a V {
        let def = &g.defs[f.id];
        match def {
            Def::Constant(v) | Def::Folded { value: v, .. } => return v,
            Def::Collection => panic!("{} is a collection; read it with `members`", g.name(f.id)),
            Def::Input(_) | Def::Derived(_) => {}
        }
        let Some(slot) = self.slot(g, f) else {
            panic!("{}: no such member", g.name(f.id));
        };
        match (def, &slot.value) {
            (_, Some(v)) => v,
            (Def::Input(default), None) => default,
            _ => unreachable!("read a rule before refreshing it"),
        }
    }

    fn refresh(&mut self, g: &Graph<V>, f: Fact) -> u64 {
        let rule = match &g.defs[f.id] {
            Def::Constant(_) | Def::Folded { .. } => return 0,
            Def::Collection => return self.collections[g.pos[f.id]].changed_at,
            Def::Input(_) => false,
            Def::Derived(_) => true,
        };
        let Some(slot) = self.slot(g, f) else {
            return REMOVED;
        };
        if !rule || (slot.value.is_some() && !slot.changed) {
            return slot.changed_at;
        }
        if slot.value.is_some()
            && let Some(changed_at) = self.try_mark_green(g, f)
        {
            return changed_at;
        }
        self.execute(g, f)
    }

    fn try_mark_green(&mut self, g: &Graph<V>, f: Fact) -> Option<u64> {
        let slot = self.slot(g, f).expect("live");
        let (since, deps) = (slot.verified_at, slot.deps.clone());
        for &d in deps.iter() {
            if self.refresh(g, d) > since {
                return None;
            }
        }
        let revision = self.revision;
        let slot = self.slot_mut(g, f).expect("live");
        slot.verified_at = revision;
        slot.changed = false;
        let changed_at = slot.changed_at;
        self.stats.marked_green += 1;
        Some(changed_at)
    }

    fn execute(&mut self, g: &Graph<V>, f: Fact) -> u64 {
        let Def::Derived(rule) = &g.defs[f.id] else {
            unreachable!()
        };
        self.frames.push(self.deps.len());
        let value = rule(&mut Context {
            graph: g,
            state: self,
            member: f.member,
            scope: g.scopes[f.id],
        });
        let start = self.frames.pop().expect("frame pushed above");
        self.stats.executed += 1;

        let revision = self.revision;
        let deps = std::mem::take(&mut self.deps);
        let read = &deps[start..];
        let slot = self.slot_mut(g, f).expect("live");
        let backdated = slot.value.as_ref() == Some(&value);
        slot.value = Some(value);
        if !backdated {
            slot.changed_at = revision;
        }
        slot.verified_at = revision;
        slot.changed = false;
        let changed_at = slot.changed_at;
        if *slot.deps != *read {
            let old = std::mem::replace(&mut slot.deps, read.into());
            self.relink(g, f, &old, read);
        }
        self.deps = deps;
        self.deps.truncate(start);
        self.stats.backdated += backdated as u64;
        changed_at
    }
}

pub struct Context<'a, V> {
    graph: &'a Graph<V>,
    state: &'a mut State<V>,
    /// The member being computed, for field rules.
    member: Option<Member>,
    scope: Option<usize>,
}

impl<V: Clone + PartialEq> Context<'_, V> {
    /// A field of this rule's own collection, named without a member, reads
    /// the member being computed.
    pub fn get(&mut self, fact: impl Into<Fact>) -> V {
        self.with(fact, V::clone)
    }

    pub fn with<R>(&mut self, fact: impl Into<Fact>, f: impl FnOnce(&V) -> R) -> R {
        let fact = self.read(fact.into());
        f(self.state.value(self.graph, fact))
    }

    pub fn is_set(&mut self, fact: impl Into<Fact>) -> bool {
        let fact = self.read(fact.into());
        self.state.is_set(self.graph, fact)
    }

    /// A collection's members; empty if unanswered.
    pub fn members(&mut self, collection: usize) -> Arc<[Member]> {
        assert!(
            matches!(self.graph.defs[collection], Def::Collection),
            "{} is not a collection",
            self.graph.name(collection)
        );
        self.record(collection.into());
        let c = self.graph.pos[collection];
        self.state.collections[c]
            .members
            .clone()
            .unwrap_or_else(|| Arc::new([]))
    }

    /// The member this rule is computing. Panics outside a field rule.
    pub fn member(&self) -> Member {
        self.member.expect("not computing a collection field")
    }

    pub fn name(&self, id: usize) -> &str {
        self.graph.name(id)
    }

    fn read(&mut self, fact: Fact) -> Fact {
        let fact = self.resolve(fact);
        self.state.refresh(self.graph, fact);
        self.record(fact);
        fact
    }

    fn record(&mut self, fact: Fact) {
        let start = *self.state.frames.last().expect("a rule is running");
        if self.state.deps.len() == start || self.state.deps.last() != Some(&fact) {
            self.state.deps.push(fact);
        }
    }

    fn resolve(&self, fact: Fact) -> Fact {
        let name = || self.graph.name(fact.id);
        match (self.graph.scopes[fact.id], fact.member) {
            (Some(c), None) if Some(c) == self.scope => Fact {
                id: fact.id,
                member: self.member,
            },
            (Some(_), None) => panic!("{}: name a member", name()),
            (None, Some(_)) => panic!("{} is not a collection field", name()),
            _ => fact,
        }
    }
}
