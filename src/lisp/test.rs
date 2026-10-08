//! Tests and scenarios for a program, written in the same Lisp.
//!
//! A scenario answers questions; a test is a scenario with expectations:
//!
//! ```
//! use factlet::lisp::{Domain, load, test::run_tests};
//!
//! let program = load(
//!     "(unit usd :prefix \"$\" :places 2)
//!      (collection w2s (input wages : usd))
//!      (def total (sum w2s wages))",
//!     &Domain::new(),
//! )
//! .unwrap();
//! let results = run_tests(
//!     &program,
//!     "total.lisp",
//!     "(test \"two W-2s\"
//!        (expect total ?)
//!        (member w2s acme (given wages $100))
//!        (member w2s globex (given wages $50))
//!        (expect total $150))",
//! )
//! .unwrap();
//! assert!(results[0].passed(), "{:?}", results[0].failures);
//! ```
//!
//! Forms run in order, so a test can answer, expect, change an answer and
//! expect again.

use super::{Diagnostic, Program, Value};
use crate::case::Case;
use crate::graph::{Fact, Kind, Member};
use crate::lisp::parser::{SExpr, SExprKind, Span, read_file};

/// One `(test "name" …)` form's outcome.
#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: String,
    /// Where the `test` form starts.
    pub span: Span,
    /// Failed expectations and malformed forms, in order.
    pub failures: Vec<Diagnostic>,
}

impl TestResult {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Run every `(test "name" form …)` in `src`, a file named `file`. A file
/// that can't be read, or has a top-level form other than `test`, is an
/// error as a whole.
pub fn run_tests(
    program: &Program,
    file: &str,
    src: &str,
) -> Result<Vec<TestResult>, Vec<Diagnostic>> {
    let forms = read_forms(file, src)?;
    let mut results = Vec::new();
    let mut errors = Vec::new();
    for form in &forms {
        let test = match head(form) {
            Some(("test", [name, body @ ..])) => match &name.kind {
                SExprKind::Str(name) => Some((name, body)),
                _ => None,
            },
            _ => None,
        };
        let Some((name, body)) = test else {
            errors.push(diagnostic(
                file,
                form.span,
                format!("expected (test \"name\" form …), found `{form}`"),
            ));
            continue;
        };
        let mut run = Run::new(program, file, true);
        run.forms(body, None);
        results.push(TestResult {
            name: name.to_string(),
            span: form.span,
            failures: run.errors,
        });
    }
    if errors.is_empty() {
        Ok(results)
    } else {
        Err(errors)
    }
}

/// A case answered by the `given`, `member` and `empty` forms in `src`, a
/// file named `file`.
pub fn scenario(program: &Program, file: &str, src: &str) -> Result<Case<Value>, Vec<Diagnostic>> {
    let forms = read_forms(file, src)?;
    let mut run = Run::new(program, file, false);
    run.forms(&forms, None);
    if run.errors.is_empty() {
        Ok(run.case)
    } else {
        Err(run.errors)
    }
}

fn read_forms(file: &str, src: &str) -> Result<Vec<SExpr>, Vec<Diagnostic>> {
    read_file(src, 0).map_err(|e| vec![diagnostic(file, e.span, e.message)])
}

fn diagnostic(file: &str, span: Span, message: String) -> Diagnostic {
    Diagnostic {
        file: file.to_string(),
        span,
        message,
    }
}

/// `(name arg …)` as its head symbol and arguments.
fn head(form: &SExpr) -> Option<(&str, &[SExpr])> {
    let SExprKind::List(items) = &form.kind else {
        return None;
    };
    let (first, rest) = items.split_first()?;
    match &first.kind {
        SExprKind::Symbol(s) => Some((s, rest)),
        _ => None,
    }
}

/// A case being answered, and checked when `expects` allows it.
struct Run<'p> {
    program: &'p Program,
    file: &'p str,
    case: Case<Value>,
    expects: bool,
    errors: Vec<Diagnostic>,
}

impl<'p> Run<'p> {
    fn new(program: &'p Program, file: &'p str, expects: bool) -> Self {
        Run {
            program,
            file,
            case: program.case(),
            expects,
            errors: Vec::new(),
        }
    }

    fn error(&mut self, span: Span, message: String) {
        self.errors.push(diagnostic(self.file, span, message));
    }

    /// Run `forms`; inside a `member` block, `member` is its collection and
    /// member, and names are that member's fields.
    fn forms(&mut self, forms: &[SExpr], member: Option<(usize, Member)>) {
        for form in forms {
            self.form(form, member);
        }
    }

    fn form(&mut self, form: &SExpr, member: Option<(usize, Member)>) {
        match head(form) {
            Some(("given", args)) => {
                for (name, value) in self.pairs(form, args) {
                    self.given(name, value, member);
                }
            }
            Some(("expect", args)) if self.expects => {
                for (name, value) in self.pairs(form, args) {
                    self.expect(name, value, member);
                }
            }
            Some(("expect-error", args)) if self.expects => {
                for name in args {
                    self.expect_error(name, member);
                }
            }
            Some(("member" | "empty", _)) if member.is_some() => {
                self.error(form.span, "collections can't be nested".into());
            }
            Some(("member", [collection, name, body @ ..])) => self.member(collection, name, body),
            Some(("empty", args)) => {
                for collection in args {
                    if let Some(c) = self.collection(collection) {
                        let result = self.case.set_empty(c);
                        if let Err(e) = result {
                            self.error(collection.span, e.to_string());
                        }
                    }
                }
            }
            _ => {
                let forms = if self.expects {
                    "given, member, empty, expect or expect-error"
                } else {
                    "given, member or empty"
                };
                self.error(form.span, format!("expected {forms}, found `{form}`"));
            }
        }
    }

    /// `name value …` as pairs; an odd one out is an error.
    fn pairs<'f>(&mut self, form: &SExpr, args: &'f [SExpr]) -> Vec<(&'f SExpr, &'f SExpr)> {
        if !args.len().is_multiple_of(2) {
            self.error(
                form.span,
                format!("expected name value pairs, found `{form}`"),
            );
        }
        args.chunks_exact(2).map(|p| (&p[0], &p[1])).collect()
    }

    /// A fact named in a form: a global, or inside a `member` block, one of
    /// its fields.
    fn fact(&mut self, name: &SExpr, member: Option<(usize, Member)>) -> Option<Fact> {
        let SExprKind::Symbol(s) = &name.kind else {
            self.error(name.span, format!("expected a fact's name, found `{name}`"));
            return None;
        };
        let graph = &self.program.graph;
        let message = match member {
            Some((c, m)) => match graph.id(&format!("{}/*/{s}", graph.name(c))) {
                Some(id) => return Some((id, m).into()),
                None => format!("`{}` has no field `{s}`", graph.name(c)),
            },
            None => match graph.id(s) {
                None => format!("unknown name `{s}`"),
                Some(id) => match (graph.collection_of(id), graph.kind(id)) {
                    (Some(c), _) => format!(
                        "`{s}` is a field of `{0}`: use it in (member {0} …)",
                        graph.name(c)
                    ),
                    (None, Kind::Collection) => {
                        format!("`{s}` is a collection: use (member {s} …) or (empty {s})")
                    }
                    _ => return Some(id.into()),
                },
            },
        };
        self.error(name.span, message);
        None
    }

    fn collection(&mut self, name: &SExpr) -> Option<usize> {
        let id = match &name.kind {
            SExprKind::Symbol(s) => self.program.id(s),
            _ => None,
        };
        match id {
            Some(id) if self.program.graph.kind(id) == Kind::Collection => Some(id),
            _ => {
                self.error(name.span, format!("`{name}` is not a collection"));
                None
            }
        }
    }

    fn given(&mut self, name: &SExpr, value: &SExpr, member: Option<(usize, Member)>) {
        let Some(fact) = self.fact(name, member) else {
            return;
        };
        if self.program.graph.kind(fact.id) != Kind::Input {
            let message = format!("`{}` is not an input", self.case.name(fact));
            return self.error(name.span, message);
        }
        let result = self.program.value_of(value).and_then(|v| {
            self.program
                .assign(&mut self.case, fact, v, &value.to_string())
        });
        if let Err(e) = result {
            self.error(value.span, e);
        }
    }

    fn expect(&mut self, name: &SExpr, value: &SExpr, member: Option<(usize, Member)>) {
        let want = match &value.kind {
            SExprKind::Symbol(s) if &**s == "?" => Ok(Value::Missing),
            _ => self.program.value_of(value),
        };
        let Some(fact) = self.fact(name, member) else {
            return;
        };
        match want {
            Err(e) => self.error(value.span, e),
            Ok(want) => {
                let got = self.case.get(fact);
                if got != want {
                    let name = self.case.name(fact);
                    self.error(value.span, format!("{name}: expected {want}, got {got}"));
                }
            }
        }
    }

    fn expect_error(&mut self, name: &SExpr, member: Option<(usize, Member)>) {
        let Some(fact) = self.fact(name, member) else {
            return;
        };
        let got = self.case.get(fact);
        if !matches!(got, Value::Error(_)) {
            let shown = self.case.name(fact);
            self.error(name.span, format!("{shown}: expected an error, got {got}"));
        }
    }

    fn member(&mut self, collection: &SExpr, name: &SExpr, body: &[SExpr]) {
        let Some(c) = self.collection(collection) else {
            return;
        };
        let member_name = match &name.kind {
            SExprKind::Symbol(s) | SExprKind::Str(s) => s,
            _ => {
                return self.error(
                    name.span,
                    format!("expected a member's name, found `{name}`"),
                );
            }
        };
        match self.case.add_member(c, member_name) {
            Ok(m) => self.forms(body, Some((c, m))),
            Err(e) => self.error(name.span, e.to_string()),
        }
    }
}
