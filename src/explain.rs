use crate::case::Case;
use crate::graph::Kind;
use std::collections::HashSet;
use std::fmt;

#[derive(Clone, Debug)]
pub struct Explanation<V> {
    pub root: Node<V>,
}

#[derive(Clone, Debug)]
pub struct Node<V> {
    pub id: usize,
    pub name: String,
    pub kind: Kind,
    pub value: V,
    pub set: bool,
    pub children: Vec<Node<V>>,
    pub repeated: bool,
}

impl<V: Clone + PartialEq> Case<V> {
    pub fn explain(&mut self, id: usize) -> Explanation<V> {
        Explanation {
            root: self.explain_node(id, &mut HashSet::new()),
        }
    }

    fn explain_node(&mut self, id: usize, seen: &mut HashSet<usize>) -> Node<V> {
        let kind = self.graph().kind(id);
        let repeated = !seen.insert(id);
        let deps = if kind == Kind::Derived && !repeated {
            self.deps(id).to_vec()
        } else {
            Vec::new()
        };
        Node {
            id,
            name: self.graph().name(id).to_string(),
            kind,
            value: self.get(id),
            set: self.is_set(id),
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
        write!(f, "{first}{} = {:?}", self.name, self.value)?;
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
