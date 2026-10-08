use crate::graph::{Def, Graph};
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stats {
    pub executed: u64,
    pub marked_green: u64,
    pub backdated: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is not an input and cannot be set", self.0)
    }
}

impl std::error::Error for Error {}

struct Slot<V> {
    value: Option<V>,
    changed_at: u64,
    verified_at: u64,
    deps: Arc<[usize]>,
}

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

impl<V: Clone + PartialEq> Case<V> {
    pub fn new(graph: Arc<Graph<V>>) -> Self {
        let empty = || Slot {
            value: None,
            changed_at: 0,
            verified_at: 0,
            deps: Arc::new([]),
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
        }
        Ok(())
    }

    pub fn get(&mut self, id: usize) -> V {
        self.state.refresh(&self.graph, id);
        self.state.value(&self.graph, id).clone()
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
}

impl<V: Clone + PartialEq> State<V> {
    fn value<'a>(&'a self, g: &'a Graph<V>, id: usize) -> &'a V {
        match (&g.defs[id], &self.slots[id].value) {
            (Def::Constant(v), _) | (_, Some(v)) => v,
            (Def::Input(default), None) => default,
            (Def::Derived(_), None) => unreachable!("read a rule before refreshing it"),
        }
    }

    fn refresh(&mut self, g: &Graph<V>, id: usize) -> u64 {
        let slot = &self.slots[id];
        match &g.defs[id] {
            Def::Constant(_) => 0,
            Def::Input(_) => slot.changed_at,
            Def::Derived(_) if slot.value.is_none() => self.execute(g, id),
            Def::Derived(_) if slot.verified_at == self.revision => slot.changed_at,
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
        if *slot.deps != self.deps[start..] {
            slot.deps = self.deps[start..].into();
        }
        self.deps.truncate(start);
        self.stats.backdated += backdated as u64;
        slot.changed_at
    }
}

pub struct Context<'a, V> {
    graph: &'a Graph<V>,
    state: &'a mut State<V>,
}

impl<V: Clone + PartialEq> Context<'_, V> {
    pub fn get(&mut self, id: usize) -> V {
        self.state.refresh(self.graph, id);
        if self.state.deps.last() != Some(&id)
            || self.state.deps.len() == *self.state.frames.last().unwrap()
        {
            self.state.deps.push(id);
        }
        self.state.value(self.graph, id).clone()
    }

    pub fn name(&self, id: usize) -> &str {
        self.graph.name(id)
    }
}
