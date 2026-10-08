//! A small Lisp for writing fact graphs as data.
//!
//! ```
//! use factlet::lisp::{Domain, load};
//!
//! let program = load(
//!     "(unit usd :prefix \"$\" :places 2)
//!      (input wages : usd)
//!      (def law/deduction $15,750)
//!      (def taxable (max $0 (- wages law/deduction)))",
//!     &Domain::new(),
//! )
//! .unwrap();
//!
//! let mut case = program.case();
//! let wages = program.id("wages").unwrap();
//! program.set(&mut case, wages, "$20,000").unwrap();
//! assert_eq!(program.get(&mut case, "taxable").unwrap().to_string(), "$4,250.00");
//! ```

mod compile;
mod cycles;
pub mod domain;
mod eval;
pub mod num;
pub mod parser;
pub mod value;

pub use compile::Diagnostic;
pub use domain::{Domain, Type};
pub use value::Value;

use crate::case::Case;
use crate::explain::Explanation;
use crate::graph::{Fact, Graph};
use crate::lisp::parser::{SExprKind, read};
use std::sync::Arc;

/// A loaded program: its compiled graph, and the types of its facts.
pub struct Program {
    pub graph: Arc<Graph<Value>>,
    pub domain: Domain,
    types: Vec<Option<Type>>,
    defaulted: Vec<usize>,
}

/// Parse, check and compile a program against `domain`.
pub fn load(src: &str, domain: &Domain) -> Result<Program, Vec<Diagnostic>> {
    Ok(Program::from(compile::compile(src, domain)?))
}

/// Load a program spread over files: `main`, and everything it reaches
/// through `(include "name")`. `read` returns a file's source by name, so
/// files can come from disk, memory or anywhere else:
///
/// ```
/// use factlet::lisp::{Domain, load_files};
///
/// let files = [
///     ("main.lisp", "(include \"law.lisp\") (input wages : number) (def tax (* law/rate wages))"),
///     ("law.lisp", "(module law (def rate 10%))"),
/// ];
/// let read = |name: &str| files.iter().find(|f| f.0 == name).map(|f| f.1.to_string());
/// let program = load_files("main.lisp", read, &Domain::new()).unwrap();
/// assert!(program.graph.is_folded(program.id("law/rate").unwrap()));
/// ```
pub fn load_files(
    main: &str,
    mut read: impl FnMut(&str) -> Option<String>,
    domain: &Domain,
) -> Result<Program, Vec<Diagnostic>> {
    Ok(Program::from(compile::compile_files(
        main, &mut read, domain,
    )?))
}

impl From<compile::Compiled> for Program {
    fn from(c: compile::Compiled) -> Program {
        Program {
            graph: c.graph,
            domain: c.domain,
            types: c.types,
            defaulted: c.defaulted,
        }
    }
}

impl Program {
    /// A fact by name; fields are `collection/*/field`.
    pub fn id(&self, name: &str) -> Option<usize> {
        self.graph.id(name)
    }

    /// A fact's type; `None` for collections.
    pub fn ty(&self, id: usize) -> Option<&Type> {
        self.types.get(id)?.as_ref()
    }

    /// Whether an input was declared with a `:default`, so it needn't be
    /// asked.
    pub fn has_default(&self, id: usize) -> bool {
        self.defaulted.contains(&id)
    }

    /// The questions blocking `fact`, as [`Case::unanswered`], less the
    /// inputs that have a default: those read their default until answered.
    pub fn unanswered(&self, case: &mut Case<Value>, fact: impl Into<Fact>) -> Vec<Fact> {
        let mut questions = case.unanswered(fact);
        questions.retain(|f| !self.has_default(f.id));
        questions
    }

    pub fn case(&self) -> Case<Value> {
        Case::new(self.graph.clone())
    }

    /// A value written as in source: `$58,000`, `22%`, `'single` (or
    /// `single`), `true`, `"text"`.
    pub fn value(&self, src: &str) -> Result<Value, String> {
        let mut forms = read(src).map_err(|e| e.to_string())?;
        if forms.len() != 1 {
            return Err(format!("expected one value, found `{src}`"));
        }
        let form = forms.remove(0);
        let variant = |name: &str| self.domain.variant(name).map(Value::Enum);
        match &form.kind {
            SExprKind::Number(lit) => Ok(Value::Num(self.domain.units.resolve(lit)?)),
            SExprKind::Str(s) => Ok(Value::Str(s.as_ref().into())),
            SExprKind::Symbol(s) if &**s == "true" => Ok(Value::Bool(true)),
            SExprKind::Symbol(s) if &**s == "false" => Ok(Value::Bool(false)),
            SExprKind::Symbol(s) => variant(s),
            SExprKind::Quote(x) => match &x.kind {
                SExprKind::Symbol(s) => variant(s),
                _ => Err(format!("expected a variant, found `{x}`")),
            },
            _ => Err(format!("expected a value, found `{src}`")),
        }
    }

    /// Answer an input from source text, checking its type.
    pub fn set(
        &self,
        case: &mut Case<Value>,
        fact: impl Into<Fact>,
        src: &str,
    ) -> Result<(), String> {
        let fact = fact.into();
        let value = self.value(src)?;
        let want = self.ty(fact.id).ok_or("not an input")?;
        let got = Type::of(&value).expect("parsed values are present");
        if !got.matches(want) {
            return Err(format!("{} is {want}, not {got} `{src}`", case.name(fact)));
        }
        case.set(fact, value).map_err(|e| e.to_string())
    }

    /// A global fact's value by name.
    pub fn get(&self, case: &mut Case<Value>, name: &str) -> Option<Value> {
        Some(case.get(self.id(name)?))
    }

    /// A global fact's derivation by name.
    pub fn explain(&self, case: &mut Case<Value>, name: &str) -> Option<Explanation<Value>> {
        Some(case.explain(self.id(name)?))
    }
}
