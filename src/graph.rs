use crate::case::Context;
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
            Def::Derived(_) => Kind::Derived,
        }
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

    pub fn build(self) -> Result<Arc<Graph<V>>, Vec<BuildError>> {
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        Ok(Arc::new(Graph {
            defs: self.defs.into(),
            names: self.names.into(),
            by_name: self.by_name,
        }))
    }
}
