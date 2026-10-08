use crate::graph::{Def, Graph, Kind};
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stats {
    pub executed: u64,
    pub marked_green: u64,
    pub backdated: u64,
    pub changed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is not an input and cannot be set", self.0)
    }
}

impl std::error::Error for Error {}

#[derive(Clone)]
struct Slot<V> {
    value: Option<V>,
    changed_at: u64,
    verified_at: u64,
    deps: Arc<[usize]>,
    dependents: Vec<usize>,
    changed: bool,
}

#[derive(Clone)]
struct State<V> {
    slots: Box<[Slot<V>]>,
    revision: u64,
    stats: Stats,
    deps: Vec<usize>,
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
        let empty = || Slot {
            value: None,
            changed_at: 0,
            verified_at: 0,
            deps: Arc::new([]),
            dependents: Vec::new(),
            changed: false,
        };
        let slots = (0..graph.len()).map(|_| empty()).collect();
        let state = State {
            slots,
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

    pub fn set(&mut self, id: usize, value: V) -> Result<(), Error> {
        self.write(id, Some(value))
    }

    pub fn unset(&mut self, id: usize) -> Result<(), Error> {
        self.write(id, None)
    }

    fn write(&mut self, id: usize, new: Option<V>) -> Result<(), Error> {
        if !matches!(self.graph.defs[id], Def::Input(_)) {
            return Err(Error(self.graph.name(id).to_string()));
        }
        let slot = &mut self.state.slots[id];
        if slot.value != new {
            self.state.revision += 1;
            slot.value = new;
            slot.changed_at = self.state.revision;
            self.state.mark_dependents(id);
        }
        Ok(())
    }

    pub fn get(&mut self, id: usize) -> V {
        self.state.refresh(&self.graph, id);
        self.state.value(&self.graph, id).clone()
    }

    pub fn is_set(&self, id: usize) -> bool {
        !matches!(self.graph.defs[id], Def::Input(_)) || self.state.slots[id].value.is_some()
    }

    pub fn deps(&mut self, id: usize) -> Arc<[usize]> {
        if let Def::Folded { deps, .. } = &self.graph.defs[id] {
            return deps.clone();
        }
        self.state.refresh(&self.graph, id);
        self.state.slots[id].deps.clone()
    }

    pub fn unanswered(&mut self, id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![id];
        while let Some(f) = stack.pop() {
            if !seen.insert(f) {
                continue;
            }
            match self.graph.kind(f) {
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
        let (g, slots) = (&*self.graph, &self.state.slots);
        for (f, slot) in slots.iter().enumerate() {
            let mut unique = slot.dependents.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                unique.len(),
                slot.dependents.len(),
                "{} lists a dependent twice",
                g.name(f)
            );
            for &d in &slot.dependents {
                assert!(
                    slots[d].deps.contains(&f),
                    "{} lists {}, which doesn't read it",
                    g.name(f),
                    g.name(d)
                );
            }
            for &d in slot.deps.iter() {
                if !g.is_fixed(d) {
                    assert!(
                        slots[d].dependents.contains(&f),
                        "{} reads {}, which doesn't list it",
                        g.name(f),
                        g.name(d)
                    );
                }
                assert!(
                    slot.changed || !slots[d].changed,
                    "clean {} reads changed {}",
                    g.name(f),
                    g.name(d)
                );
            }
        }
    }
}

impl<V: Clone + PartialEq> State<V> {
    fn mark_dependents(&mut self, id: usize) {
        let mut stack = self.slots[id].dependents.clone();
        while let Some(d) = stack.pop() {
            let slot = &mut self.slots[d];
            if !slot.changed {
                slot.changed = true;
                stack.extend_from_slice(&slot.dependents);
                self.stats.changed += 1;
            }
        }
    }

    /// Update reverse edges after `id`'s reads changed from `old` to `new`.
    fn relink(&mut self, g: &Graph<V>, id: usize, old: &[usize], new: &[usize]) {
        let sorted = |s: &[usize]| {
            let mut v = s.to_vec();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (old, new) = (sorted(old), sorted(new));
        for &d in old.iter().filter(|d| new.binary_search(d).is_err()) {
            let list = &mut self.slots[d].dependents;
            if let Some(p) = list.iter().position(|&x| x == id) {
                list.swap_remove(p);
            }
        }
        for &d in new.iter().filter(|d| old.binary_search(d).is_err()) {
            if !g.is_fixed(d) {
                self.slots[d].dependents.push(id);
            }
        }
    }

    fn value<'a>(&'a self, g: &'a Graph<V>, id: usize) -> &'a V {
        match (&g.defs[id], &self.slots[id].value) {
            (Def::Constant(v) | Def::Folded { value: v, .. }, _) | (_, Some(v)) => v,
            (Def::Input(default), None) => default,
            (Def::Derived(_), None) => unreachable!("read a rule before refreshing it"),
        }
    }

    fn refresh(&mut self, g: &Graph<V>, id: usize) -> u64 {
        let slot = &self.slots[id];
        match &g.defs[id] {
            Def::Constant(_) | Def::Folded { .. } => 0,
            Def::Input(_) => slot.changed_at,
            Def::Derived(_) if slot.value.is_none() => self.execute(g, id),
            Def::Derived(_) if !slot.changed => slot.changed_at,
            Def::Derived(_) => self
                .try_mark_green(g, id)
                .unwrap_or_else(|| self.execute(g, id)),
        }
    }

    fn try_mark_green(&mut self, g: &Graph<V>, id: usize) -> Option<u64> {
        let (since, deps) = (self.slots[id].verified_at, self.slots[id].deps.clone());
        for &d in deps.iter() {
            if self.refresh(g, d) > since {
                return None;
            }
        }
        let slot = &mut self.slots[id];
        slot.verified_at = self.revision;
        slot.changed = false;
        self.stats.marked_green += 1;
        Some(slot.changed_at)
    }

    fn execute(&mut self, g: &Graph<V>, id: usize) -> u64 {
        let Def::Derived(rule) = &g.defs[id] else {
            unreachable!()
        };
        self.frames.push(self.deps.len());
        let value = rule(&mut Context {
            graph: g,
            state: self,
        });
        let start = self.frames.pop().expect("frame pushed above");
        self.stats.executed += 1;

        let slot = &mut self.slots[id];
        let backdated = slot.value.as_ref() == Some(&value);
        slot.value = Some(value);
        if !backdated {
            slot.changed_at = self.revision;
        }
        slot.verified_at = self.revision;
        slot.changed = false;
        let changed_at = slot.changed_at;
        if *slot.deps != self.deps[start..] {
            let new: Arc<[usize]> = self.deps[start..].into();
            let old = std::mem::replace(&mut slot.deps, new.clone());
            self.relink(g, id, &old, &new);
        }
        self.deps.truncate(start);
        self.stats.backdated += backdated as u64;
        changed_at
    }
}

pub struct Context<'a, V> {
    graph: &'a Graph<V>,
    state: &'a mut State<V>,
}

impl<V: Clone + PartialEq> Context<'_, V> {
    pub fn get(&mut self, id: usize) -> V {
        self.with(id, V::clone)
    }

    pub fn with<R>(&mut self, id: usize, f: impl FnOnce(&V) -> R) -> R {
        self.read(id);
        f(self.state.value(self.graph, id))
    }

    pub fn is_set(&mut self, id: usize) -> bool {
        self.read(id);
        !matches!(self.graph.defs[id], Def::Input(_)) || self.state.slots[id].value.is_some()
    }

    fn read(&mut self, id: usize) {
        self.state.refresh(self.graph, id);
        if self.state.deps.last() != Some(&id)
            || self.state.deps.len() == *self.state.frames.last().unwrap()
        {
            self.state.deps.push(id);
        }
    }

    pub fn name(&self, id: usize) -> &str {
        self.graph.name(id)
    }
}
