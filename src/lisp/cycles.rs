//! Finding the cycles among definitions, and ordering a cycle once its
//! break points are cut. `edges[u]` lists what `u` reads.

/// The strongly connected components with a cycle: two or more nodes, or
/// one that reads itself. Each component is sorted.
pub(crate) fn cycles(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut t = Tarjan {
        edges,
        index: vec![None; edges.len()],
        low: vec![0; edges.len()],
        on_stack: vec![false; edges.len()],
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for v in 0..edges.len() {
        if t.index[v].is_none() {
            t.visit(v);
        }
    }
    t.out
        .into_iter()
        .filter(|c| c.len() > 1 || edges[c[0]].contains(&c[0]))
        .map(|mut c| {
            c.sort_unstable();
            c
        })
        .collect()
}

struct Tarjan<'a> {
    edges: &'a [Vec<usize>],
    index: Vec<Option<usize>>,
    low: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    next: usize,
    out: Vec<Vec<usize>>,
}

impl Tarjan<'_> {
    fn visit(&mut self, v: usize) {
        self.index[v] = Some(self.next);
        self.low[v] = self.next;
        self.next += 1;
        self.stack.push(v);
        self.on_stack[v] = true;
        for &w in &self.edges[v] {
            match self.index[w] {
                None => {
                    self.visit(w);
                    self.low[v] = self.low[v].min(self.low[w]);
                }
                Some(i) if self.on_stack[w] => self.low[v] = self.low[v].min(i),
                Some(_) => {}
            }
        }
        if Some(self.low[v]) == self.index[v] {
            let mut component = Vec::new();
            loop {
                let w = self.stack.pop().expect("v is on the stack");
                self.on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            self.out.push(component);
        }
    }
}

/// `nodes` ordered so each comes after the nodes it reads, ignoring reads
/// of `cut` nodes (those come from the previous round). Ties go to the
/// lower node. `None` if cutting leaves a cycle.
pub(crate) fn order(nodes: &[usize], edges: &[Vec<usize>], cut: &[usize]) -> Option<Vec<usize>> {
    let counts = |u: usize, done: &[usize]| {
        edges[u]
            .iter()
            .filter(|v| nodes.contains(v) && !cut.contains(v) && !done.contains(v))
            .count()
    };
    let mut done = Vec::new();
    while done.len() < nodes.len() {
        let next = nodes
            .iter()
            .copied()
            .filter(|u| !done.contains(u))
            .find(|&u| counts(u, &done) == 0)?;
        done.push(next);
    }
    Some(done)
}
