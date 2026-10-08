use crate::case::{Case, Context};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Constant,
    Input,
    Derived,
    Collection,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Member(pub(crate) u32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Fact {
    pub id: usize,
    pub member: Option<Member>,
}

impl From<usize> for Fact {
    fn from(id: usize) -> Fact {
        Fact { id, member: None }
    }
}

impl From<(usize, Member)> for Fact {
    fn from((id, member): (usize, Member)) -> Fact {
        Fact {
            id,
            member: Some(member),
        }
    }
}

impl PartialEq<usize> for Fact {
    fn eq(&self, id: &usize) -> bool {
        self.id == *id && self.member.is_none()
    }
}

pub(crate) type Rule<V> = Arc<dyn Fn(&mut Context<'_, V>) -> V + Send + Sync>;

pub(crate) enum Def<V> {
    Constant(V),
    Input(V),
    Derived(Rule<V>),
    Folded { value: V, deps: Arc<[Fact]> },
    Collection,
}

pub struct Graph<V> {
    pub(crate) defs: Box<[Def<V>]>,
    names: Box<[Box<str>]>,
    by_name: HashMap<Box<str>, usize>,
    pub(crate) scopes: Box<[Option<usize>]>,
    /// A field's index in its member rows, or a collection's index in
    /// `collections`.
    pub(crate) pos: Box<[usize]>,
    /// Each collection's fields in row order.
    pub(crate) collections: Box<[Box<[usize]>]>,
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
            Def::Collection => Kind::Collection,
        }
    }

    pub fn collection_of(&self, id: usize) -> Option<usize> {
        self.scopes[id]
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
    NotACollection { field: String, scope: String },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::DuplicateName(n) => write!(f, "fact {n:?} defined twice"),
            BuildError::NotACollection { field, scope } => {
                write!(f, "{field}: {scope} is not a collection")
            }
        }
    }
}

impl std::error::Error for BuildError {}

pub struct Builder<V> {
    defs: Vec<Def<V>>,
    names: Vec<Box<str>>,
    scopes: Vec<Option<usize>>,
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
            scopes: Vec::new(),
            by_name: HashMap::new(),
            errors: Vec::new(),
        }
    }

    pub fn constant(&mut self, name: &str, value: V) -> usize {
        self.define(name, None, Def::Constant(value))
    }

    pub fn input(&mut self, name: &str, default: V) -> usize {
        self.define(name, None, Def::Input(default))
    }

    pub fn derived(
        &mut self,
        name: &str,
        rule: impl Fn(&mut Context<'_, V>) -> V + Send + Sync + 'static,
    ) -> usize {
        self.define(name, None, Def::Derived(Arc::new(rule)))
    }

    pub fn collection(&mut self, name: &str) -> usize {
        self.define(name, None, Def::Collection)
    }

    pub fn field_input(&mut self, collection: usize, name: &str, default: V) -> usize {
        self.define(name, Some(collection), Def::Input(default))
    }

    /// Inside the rule, a bare field of `collection` reads the member being
    /// computed.
    pub fn field_derived(
        &mut self,
        collection: usize,
        name: &str,
        rule: impl Fn(&mut Context<'_, V>) -> V + Send + Sync + 'static,
    ) -> usize {
        self.define(name, Some(collection), Def::Derived(Arc::new(rule)))
    }

    fn define(&mut self, name: &str, scope: Option<usize>, def: Def<V>) -> usize {
        let id = self.defs.len();
        let name: Box<str> = match scope {
            Some(c) => format!("{}/*/{name}", self.names[c]).into(),
            None => name.into(),
        };
        if let Some(c) = scope
            && !matches!(self.defs[c], Def::Collection)
        {
            self.errors.push(BuildError::NotACollection {
                field: name.to_string(),
                scope: self.names[c].to_string(),
            });
        }
        if self.by_name.insert(name.clone(), id).is_some() {
            self.errors
                .push(BuildError::DuplicateName(name.to_string()));
        }
        self.names.push(name);
        self.scopes.push(scope);
        self.defs.push(def);
        id
    }
}

impl<V: Clone + PartialEq> Builder<V> {
    pub fn build(self) -> Result<Arc<Graph<V>>, Vec<BuildError>> {
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        let mut pos = vec![0; self.defs.len()];
        let mut collections: Vec<Vec<usize>> = Vec::new();
        for (id, def) in self.defs.iter().enumerate() {
            if let Some(c) = self.scopes[id] {
                pos[id] = collections[pos[c]].len();
                collections[pos[c]].push(id);
            } else if matches!(def, Def::Collection) {
                pos[id] = collections.len();
                collections.push(Vec::new());
            }
        }
        let probe = Arc::new(Graph {
            defs: self.defs.into(),
            names: self.names.into(),
            by_name: self.by_name,
            scopes: self.scopes.into(),
            pos: pos.into(),
            collections: collections.into_iter().map(Vec::into).collect(),
        });

        // Ids are in dependency order, so one pass finds every rule that
        // reads only constants and rules already folded.
        let mut case = Case::new(probe.clone());
        let mut fold = vec![false; probe.len()];
        let mut folded = Vec::new();
        for id in 0..probe.len() {
            if !matches!(probe.defs[id], Def::Derived(_)) || probe.scopes[id].is_some() {
                continue;
            }
            let deps = case.deps(id);
            if deps.iter().all(|d| fold[d.id] || probe.is_fixed(d.id)) {
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
