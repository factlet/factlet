//! A tax domain for `factlet::lisp`: dollars (`$1,234.56`) and
//! `(brackets income schedule)`.

use factlet::lisp::value::{Quantity, UnitDef};
use factlet::lisp::{Domain, Type, Value};

pub fn domain() -> Domain {
    let mut d = Domain::new();
    d.unit(UnitDef {
        name: "usd".into(),
        prefix: Some("$".into()),
        suffix: None,
        places: 2,
    })
    .expect("fresh domain");
    d.builtin("brackets", check_brackets, eval_brackets);
    d
}

/// `(brackets income [rate edge rate edge … rate])`: each rate applies to
/// income up to the following edge, the last rate to everything above.
fn check_brackets(args: &[Type]) -> Result<Type, String> {
    let usage = "expected (brackets amount [rate edge … rate])";
    let [Type::Num(Some(unit)), Type::List(schedule)] = args else {
        return Err(usage.into());
    };
    if schedule.len() % 2 == 0 {
        return Err("the schedule must end with a rate".into());
    }
    for (i, t) in schedule.iter().enumerate() {
        let want = if i % 2 == 0 {
            Type::Num(None)
        } else {
            Type::Num(Some(unit.clone()))
        };
        if *t != want {
            return Err(format!("schedule item {} is {t}, expected {want}", i + 1));
        }
    }
    Ok(Type::Num(Some(unit.clone())))
}

fn eval_brackets(args: &[Value]) -> Value {
    let (Value::Num(income), Value::List(schedule)) = (&args[0], &args[1]) else {
        unreachable!("checked")
    };
    let num = |v: &Value| match v {
        Value::Num(q) => q.clone(),
        _ => unreachable!("checked"),
    };
    let zero = Quantity {
        num: factlet::lisp::num::Num::ZERO,
        unit: income.unit.clone(),
    };
    let (mut tax, mut lower) = (zero.clone(), zero);
    let step = |tax: &Quantity, lower: &Quantity, upper: Option<Quantity>, rate: Quantity| {
        let top = match upper {
            Some(u) => income.min(&u)?,
            None => income.clone(),
        };
        if top.compare(lower)?.is_le() {
            return Ok(tax.clone());
        }
        tax.add(&top.sub(lower)?.mul(&rate)?)
    };
    for pair in schedule.chunks(2) {
        let upper = pair.get(1).map(num);
        match step(&tax, &lower, upper.clone(), num(&pair[0])) {
            Ok(t) => tax = t,
            Err(e) => return Value::Error(format!("{e}").into()),
        }
        if let Some(u) = upper {
            lower = u;
        }
    }
    Value::Num(tax)
}
