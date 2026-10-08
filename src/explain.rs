use crate::case::Case;
use crate::graph::{Fact, Kind};
use std::collections::HashSet;
use std::fmt;

#[derive(Clone, Debug)]
pub struct Explanation<V> {
    pub root: Node<V>,
}

#[derive(Clone, Debug)]
pub struct Node<V> {
    pub fact: Fact,
    pub name: String,
    pub kind: Kind,
    /// `None` for collections.
    pub value: Option<V>,
    pub set: bool,
    pub children: Vec<Node<V>>,
    pub repeated: bool,
}

impl<V: Clone + PartialEq> Case<V> {
    pub fn explain(&mut self, fact: impl Into<Fact>) -> Explanation<V> {
        Explanation {
            root: self.explain_node(fact.into(), &mut HashSet::new()),
        }
    }

    fn explain_node(&mut self, fact: Fact, seen: &mut HashSet<Fact>) -> Node<V> {
        let kind = self.graph().kind(fact.id);
        let repeated = !seen.insert(fact);
        let deps = if kind == Kind::Derived && !repeated {
            self.deps(fact).to_vec()
        } else {
            Vec::new()
        };
        Node {
            fact,
            name: self.name(fact),
            kind,
            value: (kind != Kind::Collection).then(|| self.get(fact)),
            set: self.is_set(fact),
            children: deps
                .into_iter()
                .map(|d| self.explain_node(d, seen))
                .collect(),
            repeated,
        }
    }
}

impl<V: fmt::Debug> fmt::Display for Explanation<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.root.render(f, "", "")
    }
}

impl<V: fmt::Debug> Node<V> {
    fn render(&self, f: &mut fmt::Formatter<'_>, first: &str, rest: &str) -> fmt::Result {
        write!(f, "{first}{}", self.name)?;
        if let Some(v) = &self.value {
            write!(f, " = {v:?}")?;
        }
        if !self.set {
            f.write_str("  (unanswered)")?;
        }
        if self.repeated {
            f.write_str("  (see above)")?;
        }
        writeln!(f)?;
        for (i, child) in self.children.iter().enumerate() {
            let last = i + 1 == self.children.len();
            let (branch, indent) = if last {
                ("└── ", "    ")
            } else {
                ("├── ", "│   ")
            };
            child.render(f, &format!("{rest}{branch}"), &format!("{rest}{indent}"))?;
        }
        Ok(())
    }
}
