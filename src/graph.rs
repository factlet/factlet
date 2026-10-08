use crate::case::{Case, Context};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Constant,
    Input,
    Derived,
}

pub(crate) type Rule<V> = Arc<dyn Fn(&mut Context<'_, V>) -> V + Send + Sync>;

pub(crate) enum Def<V> {
    Constant(V),
    Input(V),
    Derived(Rule<V>),
    Folded { value: V, deps: Arc<[usize]> },
}

pub struct Graph<V> {
    pub(crate) defs: Box<[Def<V>]>,
    names: Box<[Box<str>]>,
    by_name: HashMap<Box<str>, usize>,
}

impl<V> Graph<V> {
    pub fn builder() -> Builder<V> {
        Builder::new()
    }
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn id(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    pub fn name(&self, id: usize) -> &str {
        &self.names[id]
    }

    pub fn kind(&self, id: usize) -> Kind {
        match &self.defs[id] {
            Def::Constant(_) => Kind::Constant,
            Def::Input(_) => Kind::Input,
            Def::Derived(_) | Def::Folded { .. } => Kind::Derived,
        }
    }

    pub fn is_folded(&self, id: usize) -> bool {
        matches!(self.defs[id], Def::Folded { .. })
    }

    pub fn folded(&self) -> usize {
        (0..self.len()).filter(|&id| self.is_folded(id)).count()
    }

    pub(crate) fn is_fixed(&self, id: usize) -> bool {
        matches!(self.defs[id], Def::Constant(_) | Def::Folded { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    DuplicateName(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::DuplicateName(n) => write!(f, "fact {n:?} defined twice"),
        }
    }
}

impl std::error::Error for BuildError {}

pub struct Builder<V> {
    defs: Vec<Def<V>>,
    names: Vec<Box<str>>,
    by_name: HashMap<Box<str>, usize>,
    errors: Vec<BuildError>,
}

impl<V> Default for Builder<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> Builder<V> {
    fn new() -> Self {
        Builder {
            defs: Vec::new(),
            names: Vec::new(),
            by_name: HashMap::new(),
            errors: Vec::new(),
        }
    }

    pub fn constant(&mut self, name: &str, value: V) -> usize {
        self.define(name, Def::Constant(value))
    }

    pub fn input(&mut self, name: &str, default: V) -> usize {
        self.define(name, Def::Input(default))
    }

    pub fn derived(
        &mut self,
        name: &str,
        rule: impl Fn(&mut Context<'_, V>) -> V + Send + Sync + 'static,
    ) -> usize {
        self.define(name, Def::Derived(Arc::new(rule)))
    }

    fn define(&mut self, name: &str, def: Def<V>) -> usize {
        let id = self.defs.len();
        if self.by_name.insert(name.into(), id).is_some() {
            self.errors
                .push(BuildError::DuplicateName(name.to_string()));
        }
        self.names.push(name.into());
        self.defs.push(def);
        id
    }
}

impl<V: Clone + PartialEq> Builder<V> {
    pub fn build(self) -> Result<Arc<Graph<V>>, Vec<BuildError>> {
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        let probe = Arc::new(Graph {
            defs: self.defs.into(),
            names: self.names.into(),
            by_name: self.by_name,
        });

        // Ids are in dependency order, so one pass finds every rule that
        // reads only constants and rules already folded.
        let mut case = Case::new(probe.clone());
        let mut fold = vec![false; probe.len()];
        let mut folded = Vec::new();
        for id in 0..probe.len() {
            if !matches!(probe.defs[id], Def::Derived(_)) {
                continue;
            }
            let deps = case.deps(id);
            if deps.iter().all(|&d| fold[d] || probe.is_fixed(d)) {
                fold[id] = true;
                folded.push((id, case.get(id), deps));
            }
        }
        drop(case);

        let mut graph = Arc::into_inner(probe).expect("probe case dropped");
        let mut defs = graph.defs.into_vec();
        for (id, value, deps) in folded {
            defs[id] = Def::Folded { value, deps };
        }
        graph.defs = defs.into();
        Ok(Arc::new(graph))
    }
}
