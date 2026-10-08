use crate::lisp::value::{EnumDef, Unit, UnitDef, Units, Value, Variant};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// A value's static type. Every type also admits `Missing`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Type {
    /// A number in a unit, or dimensionless.
    Num(Option<Unit>),
    Bool,
    Str,
    Date,
    Enum(Arc<EnumDef>),
    /// A fixed-shape list, such as a bracket schedule; `[usd 12]` is twelve
    /// `usd`s.
    List(Arc<[Type]>),
    /// After an error, so one mistake isn't reported again downstream.
    Unknown,
}

impl Type {
    /// A list's element type, if every element has the same one.
    pub fn element(&self) -> Option<&Type> {
        match self {
            Type::List(items) => {
                let first = items.first()?;
                items.iter().all(|t| t.matches(first)).then_some(first)
            }
            _ => None,
        }
    }

    /// Equal, or either is `Unknown`.
    pub fn matches(&self, o: &Type) -> bool {
        match (self, o) {
            (Type::Unknown, _) | (_, Type::Unknown) => true,
            (Type::List(a), Type::List(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| a.matches(b))
            }
            _ => self == o,
        }
    }

    /// The type of a present value.
    pub fn of(value: &Value) -> Option<Type> {
        Some(match value {
            Value::Missing | Value::Error(_) => return None,
            Value::Bool(_) => Type::Bool,
            Value::Num(q) => Type::Num(q.unit.clone()),
            Value::Str(_) => Type::Str,
            Value::Date(_) => Type::Date,
            Value::Enum(v) => Type::Enum(v.ty().clone()),
            Value::List(items) => Type::List(items.iter().map(Type::of).collect::<Option<_>>()?),
        })
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Num(None) => f.write_str("number"),
            Type::Num(Some(u)) => f.write_str(u.name()),
            Type::Bool => f.write_str("bool"),
            Type::Str => f.write_str("string"),
            Type::Date => f.write_str("date"),
            Type::Enum(e) => f.write_str(&e.name),
            Type::List(items) if items.len() > 1 && items.iter().all(|t| *t == items[0]) => {
                write!(f, "[{} {}]", items[0], items.len())
            }
            Type::List(items) => {
                f.write_str("[")?;
                for (i, t) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{t}")?;
                }
                f.write_str("]")
            }
            Type::Unknown => f.write_str("?"),
        }
    }
}

pub type CheckFn = dyn Fn(&[Type]) -> Result<Type, String> + Send + Sync;
pub type EvalFn = dyn Fn(&[Value]) -> Value + Send + Sync;

/// A function supplied from Rust. `check` types a call from its argument
/// types when the program loads; `eval` only ever sees present values
/// (a `Missing` or `Error` argument skips the call and propagates).
#[derive(Clone)]
pub struct Builtin {
    pub(crate) check: Arc<CheckFn>,
    pub(crate) eval: Arc<EvalFn>,
}

/// What a program can talk about beyond the core language: units (and so
/// literal prefixes like `$`), enumerations, and functions. Programs can
/// also declare units and enums themselves with `(unit …)` and `(enum …)`.
#[derive(Clone, Default)]
pub struct Domain {
    pub units: Units,
    enums: Vec<Arc<EnumDef>>,
    builtins: HashMap<Box<str>, Builtin>,
}

impl Domain {
    pub fn new() -> Domain {
        Domain::default()
    }

    pub fn unit(&mut self, def: UnitDef) -> Result<Unit, String> {
        if self.ty(&def.name).is_some() {
            return Err(format!("type `{}` defined twice", def.name));
        }
        self.units.define(def)
    }

    pub fn enumeration(&mut self, name: &str, variants: &[&str]) -> Result<Arc<EnumDef>, String> {
        if self.ty(name).is_some() {
            return Err(format!("type `{name}` defined twice"));
        }
        for (i, v) in variants.iter().enumerate() {
            if variants[..i].contains(v) {
                return Err(format!("`{v}` listed twice in `{name}`"));
            }
            if let Ok(other) = self.variant(v) {
                return Err(format!("`{v}` is already a `{}`", other.ty().name));
            }
        }
        let def = Arc::new(EnumDef {
            name: name.into(),
            variants: variants.iter().map(|&v| v.into()).collect(),
        });
        self.enums.push(def.clone());
        Ok(def)
    }

    /// Register a function callable as `(name args…)`.
    pub fn builtin(
        &mut self,
        name: &str,
        check: impl Fn(&[Type]) -> Result<Type, String> + Send + Sync + 'static,
        eval: impl Fn(&[Value]) -> Value + Send + Sync + 'static,
    ) {
        let builtin = Builtin {
            check: Arc::new(check),
            eval: Arc::new(eval),
        };
        self.builtins.insert(name.into(), builtin);
    }

    pub(crate) fn get_builtin(&self, name: &str) -> Option<&Builtin> {
        self.builtins.get(name)
    }

    /// A type by name: `number`, `bool`, `string`, a unit or an enum.
    pub fn ty(&self, name: &str) -> Option<Type> {
        match name {
            "number" => return Some(Type::Num(None)),
            "bool" => return Some(Type::Bool),
            "string" => return Some(Type::Str),
            "date" => return Some(Type::Date),
            _ => {}
        }
        if let Some(u) = self.units.get(name) {
            return Some(Type::Num(Some(u.clone())));
        }
        let e = self.enums.iter().find(|e| &*e.name == name)?;
        Some(Type::Enum(e.clone()))
    }

    /// A variant by name; names are unique across enums.
    pub fn variant(&self, name: &str) -> Result<Variant, String> {
        for e in &self.enums {
            if let Some(index) = e.variants.iter().position(|v| &**v == name) {
                return Ok(Variant {
                    ty: e.clone(),
                    index,
                });
            }
        }
        Err(format!("unknown variant `{name}`"))
    }
}
