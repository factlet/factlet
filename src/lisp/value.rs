use crate::lisp::num::Num;
use crate::lisp::parser::NumLit;
use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

/// How a unit is written: `$1,234.50` is prefix `$`, 2 places.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnitDef {
    pub name: Box<str>,
    pub prefix: Option<Box<str>>,
    pub suffix: Option<Box<str>>,
    /// Decimal places shown; values themselves stay exact.
    pub places: u32,
}

/// A unit, holding its definition so values can format themselves.
#[derive(Clone)]
pub struct Unit(Arc<UnitDef>);

impl Unit {
    pub fn def(&self) -> &UnitDef {
        &self.0
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }
}

impl PartialEq for Unit {
    fn eq(&self, o: &Unit) -> bool {
        self.0.name == o.0.name
    }
}

impl Eq for Unit {}

impl fmt::Debug for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The units a program knows, and what literal prefixes and suffixes mean.
#[derive(Clone, Default)]
pub struct Units {
    units: Vec<Unit>,
}

impl Units {
    pub fn define(&mut self, def: UnitDef) -> Result<Unit, String> {
        for u in &self.units {
            let u = u.def();
            if u.name == def.name {
                return Err(format!("unit `{}` defined twice", def.name));
            }
            for (affix, theirs) in [(&def.prefix, &u.prefix), (&def.suffix, &u.suffix)] {
                if let Some(a) = affix
                    && theirs.as_ref() == Some(a)
                {
                    return Err(format!("`{a}` already means `{}`", u.name));
                }
            }
        }
        if def.suffix.as_deref() == Some("%") {
            return Err("`%` is reserved for percentages".into());
        }
        let unit = Unit(Arc::new(def));
        self.units.push(unit.clone());
        Ok(unit)
    }

    pub fn get(&self, name: &str) -> Option<&Unit> {
        self.units.iter().find(|u| u.name() == name)
    }

    /// What a literal means: `22%` is 0.22, `$5` is 5 of whichever unit has
    /// prefix `$`.
    pub fn resolve(&self, lit: &NumLit) -> Result<Quantity, String> {
        let num = Num::from_lit(lit).ok_or_else(|| format!("`{lit}` is too large"))?;
        let (prefix, suffix) = (lit.prefix.as_deref(), lit.suffix.as_deref());
        if suffix == Some("%") {
            if prefix.is_some() {
                return Err(format!("`{lit}`: a percentage can't have a unit"));
            }
            let num = num.checked_div(Num::int(100)).ok_or("too large")?;
            return Ok(Quantity { num, unit: None });
        }
        let find = |affix: Option<&str>, which: &str, get: fn(&UnitDef) -> Option<&str>| {
            affix
                .map(|a| {
                    self.units
                        .iter()
                        .find(|u| get(u.def()) == Some(a))
                        .ok_or_else(|| format!("`{lit}`: unknown unit {which} `{a}`"))
                })
                .transpose()
        };
        let by_prefix = find(prefix, "prefix", |u| u.prefix.as_deref())?;
        let by_suffix = find(suffix, "suffix", |u| u.suffix.as_deref())?;
        let unit = match (by_prefix, by_suffix) {
            (Some(p), Some(s)) if p != s => {
                return Err(format!("`{lit}`: both {} and {}", p.name(), s.name()));
            }
            (p, s) => p.or(s).cloned(),
        };
        Ok(Quantity { num, unit })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueError {
    UnitMismatch {
        op: &'static str,
        left: Option<Unit>,
        right: Option<Unit>,
    },
    Overflow,
    DivByZero,
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |u: &Option<Unit>| u.as_ref().map_or("a plain number", Unit::name).to_string();
        match self {
            ValueError::UnitMismatch { op, left, right } => {
                write!(f, "`{op}` can't combine {} and {}", name(left), name(right))
            }
            ValueError::Overflow => f.write_str("number too large"),
            ValueError::DivByZero => f.write_str("division by zero"),
        }
    }
}

impl std::error::Error for ValueError {}

/// A number and its unit; `None` is dimensionless (`22%` is 0.22).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Quantity {
    pub num: Num,
    pub unit: Option<Unit>,
}

impl Quantity {
    pub fn plain(num: Num) -> Quantity {
        Quantity { num, unit: None }
    }

    pub fn of(num: Num, unit: &Unit) -> Quantity {
        Quantity {
            num,
            unit: Some(unit.clone()),
        }
    }

    fn mismatch(&self, op: &'static str, o: &Quantity) -> ValueError {
        ValueError::UnitMismatch {
            op,
            left: self.unit.clone(),
            right: o.unit.clone(),
        }
    }

    fn same_unit(&self, op: &'static str, o: &Quantity) -> Result<(), ValueError> {
        if self.unit == o.unit {
            Ok(())
        } else {
            Err(self.mismatch(op, o))
        }
    }

    fn with(&self, num: Option<Num>) -> Result<Quantity, ValueError> {
        Ok(Quantity {
            num: num.ok_or(ValueError::Overflow)?,
            unit: self.unit.clone(),
        })
    }

    pub fn add(&self, o: &Quantity) -> Result<Quantity, ValueError> {
        self.same_unit("+", o)?;
        self.with(self.num.checked_add(o.num))
    }

    pub fn sub(&self, o: &Quantity) -> Result<Quantity, ValueError> {
        self.same_unit("-", o)?;
        self.with(self.num.checked_sub(o.num))
    }

    pub fn mul(&self, o: &Quantity) -> Result<Quantity, ValueError> {
        let unit = match (&self.unit, &o.unit) {
            (Some(_), Some(_)) => return Err(self.mismatch("*", o)),
            (u, None) | (None, u) => u.clone(),
        };
        let num = self.num.checked_mul(o.num).ok_or(ValueError::Overflow)?;
        Ok(Quantity { num, unit })
    }

    pub fn div(&self, o: &Quantity) -> Result<Quantity, ValueError> {
        let unit = match (&self.unit, &o.unit) {
            (a, b) if a == b => None,
            (a, None) => a.clone(),
            _ => return Err(self.mismatch("/", o)),
        };
        if o.num.is_zero() {
            return Err(ValueError::DivByZero);
        }
        let num = self.num.checked_div(o.num).ok_or(ValueError::Overflow)?;
        Ok(Quantity { num, unit })
    }

    pub fn compare(&self, o: &Quantity) -> Result<Ordering, ValueError> {
        self.same_unit("compare", o)?;
        Ok(self.num.cmp(&o.num))
    }

    pub fn min(&self, o: &Quantity) -> Result<Quantity, ValueError> {
        self.same_unit("min", o)?;
        Ok(if o.num < self.num { o } else { self }.clone())
    }

    pub fn max(&self, o: &Quantity) -> Result<Quantity, ValueError> {
        self.same_unit("max", o)?;
        Ok(if o.num > self.num { o } else { self }.clone())
    }

    /// To the nearest multiple of `step`, halves away from zero. `step` is
    /// in the same unit, or a plain number.
    pub fn round(&self, step: &Quantity) -> Result<Quantity, ValueError> {
        self.to_step("round", step, Num::round)
    }

    pub fn floor(&self, step: &Quantity) -> Result<Quantity, ValueError> {
        self.to_step("floor", step, Num::floor)
    }

    pub fn ceil(&self, step: &Quantity) -> Result<Quantity, ValueError> {
        self.to_step("ceil", step, Num::ceil)
    }

    fn to_step(
        &self,
        op: &'static str,
        step: &Quantity,
        f: fn(Num, Num) -> Option<Num>,
    ) -> Result<Quantity, ValueError> {
        if step.unit.is_some() && step.unit != self.unit {
            return Err(self.mismatch(op, step));
        }
        if step.num.is_zero() {
            return Err(ValueError::DivByZero);
        }
        self.with(f(self.num, step.num))
    }
}

/// `-$1,234.50` for a unit with prefix `$` and 2 places; exact for plain
/// numbers.
impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(unit) = &self.unit else {
            return write!(f, "{}", self.num);
        };
        let u = unit.def();
        let fixed = self.num.fmt_fixed(u.places).ok_or(fmt::Error)?;
        let (sign, fixed) = match fixed.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => ("", &fixed[..]),
        };
        let (int, frac) = fixed.split_at(fixed.find('.').unwrap_or(fixed.len()));
        let mut grouped = String::new();
        for (i, ch) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        let (prefix, suffix) = (u.prefix.as_deref(), u.suffix.as_deref());
        write!(
            f,
            "{sign}{}{grouped}{frac}{}",
            prefix.unwrap_or_default(),
            suffix.unwrap_or_default()
        )
    }
}

/// An enumeration such as filing status.
#[derive(PartialEq, Eq, Debug)]
pub struct EnumDef {
    pub name: Box<str>,
    pub variants: Box<[Box<str>]>,
}

/// One variant of an enumeration.
#[derive(Clone)]
pub struct Variant {
    pub(crate) ty: Arc<EnumDef>,
    pub(crate) index: usize,
}

impl Variant {
    pub fn ty(&self) -> &Arc<EnumDef> {
        &self.ty
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn name(&self) -> &str {
        &self.ty.variants[self.index]
    }
}

impl PartialEq for Variant {
    fn eq(&self, o: &Variant) -> bool {
        self.index == o.index && self.ty.name == o.ty.name
    }
}

impl Eq for Variant {}

/// A fact's value in a Lisp program. `Missing` is an unanswered input, or
/// anything computed from one; `Error` (division by zero, overflow)
/// propagates the same way.
#[derive(Clone, PartialEq, Eq)]
pub enum Value {
    Missing,
    Error(Arc<str>),
    Bool(bool),
    Num(Quantity),
    Str(Arc<str>),
    Enum(Variant),
    List(Arc<[Value]>),
}

impl Value {
    /// `Missing` or `Error`.
    pub fn is_absent(&self) -> bool {
        matches!(self, Value::Missing | Value::Error(_))
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Missing => f.write_str("?"),
            Value::Error(e) => write!(f, "<error: {e}>"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(q) => write!(f, "{q}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Enum(v) => f.write_str(v.name()),
            Value::List(items) => {
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
        }
    }
}

/// The same as `Display`, so `explain` trees and test failures read like
/// the source: `$58,000.00`, not `Num(Quantity { .. })`.
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
