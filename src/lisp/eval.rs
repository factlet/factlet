use crate::case::Context;
use crate::graph::Member;
use crate::lisp::domain::EvalFn;
use crate::lisp::value::{Quantity, Value, ValueError};
use std::cmp::Ordering;
use std::sync::Arc;

/// A checked expression, with names resolved to fact ids.
#[derive(Clone)]
pub(crate) enum Expr {
    Lit(Value),
    Global(usize),
    /// A collection field: the member an enclosing aggregate is visiting,
    /// or else the member a field rule is computing.
    Field {
        id: usize,
        collection: usize,
    },
    /// A `let` binding, by position.
    Var(usize),
    Let(Vec<Expr>, Box<Expr>),
    If(Box<[Expr; 3]>),
    Cond(Vec<(Expr, Expr)>, Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Given(Box<Expr>),
    OrElse(Box<[Expr; 2]>),
    Op(Op, Vec<Expr>),
    /// `arms[index[variant]]`.
    Table {
        key: Box<Expr>,
        index: Vec<usize>,
        arms: Vec<Expr>,
    },
    Agg {
        agg: Agg,
        collection: usize,
        body: Option<Box<Expr>>,
    },
    Call(Arc<EvalFn>, Vec<Expr>),
    List(Vec<Expr>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Op {
    Add,
    Sub,
    Neg,
    Mul,
    Div,
    Min,
    Max,
    Abs,
    Round,
    Floor,
    Ceil,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone)]
pub(crate) enum Agg {
    /// With the zero of the body's unit, for no members.
    Sum(Value),
    Count,
    Any,
    All,
    MinOf,
    MaxOf,
}

#[derive(Default)]
pub(crate) struct Env {
    vars: Vec<Value>,
    /// Members being visited by enclosing aggregates, innermost last.
    members: Vec<(usize, Member)>,
}

fn num(v: &Value) -> &Quantity {
    match v {
        Value::Num(q) => q,
        other => unreachable!("checked as a number, got {other}"),
    }
}

fn result(r: Result<Quantity, ValueError>) -> Value {
    r.map_or_else(|e| Value::Error(e.to_string().into()), Value::Num)
}

/// The first `Error`, else `Missing` if any is missing.
fn absent(values: &[Value]) -> Option<Value> {
    let mut missing = false;
    for v in values {
        match v {
            Value::Error(_) => return Some(v.clone()),
            Value::Missing => missing = true,
            _ => {}
        }
    }
    missing.then_some(Value::Missing)
}

pub(crate) fn eval(e: &Expr, cx: &mut Context<'_, Value>, env: &mut Env) -> Value {
    match e {
        Expr::Lit(v) => v.clone(),
        Expr::Global(id) => cx.get(*id),
        Expr::Field { id, collection } => {
            match env.members.iter().rev().find(|(c, _)| c == collection) {
                Some(&(_, m)) => cx.get((*id, m)),
                None => cx.get(*id),
            }
        }
        Expr::Var(i) => env.vars[*i].clone(),
        Expr::Let(values, body) => {
            let depth = env.vars.len();
            for v in values {
                let v = eval(v, cx, env);
                env.vars.push(v);
            }
            let out = eval(body, cx, env);
            env.vars.truncate(depth);
            out
        }
        Expr::If(branches) => {
            let [test, then, otherwise] = &**branches;
            match eval(test, cx, env) {
                Value::Bool(true) => eval(then, cx, env),
                Value::Bool(false) => eval(otherwise, cx, env),
                absent => absent,
            }
        }
        Expr::Cond(arms, otherwise) => {
            for (test, then) in arms {
                match eval(test, cx, env) {
                    Value::Bool(true) => return eval(then, cx, env),
                    Value::Bool(false) => {}
                    absent => return absent,
                }
            }
            eval(otherwise, cx, env)
        }
        Expr::And(items) => kleene(items, false, cx, env),
        Expr::Or(items) => kleene(items, true, cx, env),
        Expr::Not(x) => match eval(x, cx, env) {
            Value::Bool(b) => Value::Bool(!b),
            absent => absent,
        },
        Expr::Given(x) => match eval(x, cx, env) {
            Value::Missing => Value::Bool(false),
            e @ Value::Error(_) => e,
            _ => Value::Bool(true),
        },
        Expr::OrElse(pair) => match eval(&pair[0], cx, env) {
            Value::Missing => eval(&pair[1], cx, env),
            v => v,
        },
        Expr::Op(op, args) => {
            // Strict: read every argument before giving up, so `unanswered`
            // reports all of them.
            let values: Vec<Value> = args.iter().map(|a| eval(a, cx, env)).collect();
            absent(&values).unwrap_or_else(|| apply(*op, &values))
        }
        Expr::Table { key, index, arms } => match eval(key, cx, env) {
            Value::Enum(v) => eval(&arms[index[v.index()]], cx, env),
            absent => absent,
        },
        Expr::Agg {
            agg,
            collection,
            body,
        } => aggregate(agg, *collection, body.as_deref(), cx, env),
        Expr::Call(f, args) => {
            let values: Vec<Value> = args.iter().map(|a| eval(a, cx, env)).collect();
            absent(&values).unwrap_or_else(|| f(&values))
        }
        Expr::List(items) => {
            let values: Vec<Value> = items.iter().map(|a| eval(a, cx, env)).collect();
            absent(&values).unwrap_or_else(|| Value::List(values.into()))
        }
    }
}

/// `and` (stop at false) or `or` (stop at true); otherwise `Missing` if
/// any operand was.
fn kleene(items: &[Expr], stop: bool, cx: &mut Context<'_, Value>, env: &mut Env) -> Value {
    let mut missing = false;
    for item in items {
        match eval(item, cx, env) {
            Value::Bool(b) if b == stop => return Value::Bool(stop),
            Value::Bool(_) => {}
            Value::Missing => missing = true,
            error => return error,
        }
    }
    if missing {
        Value::Missing
    } else {
        Value::Bool(!stop)
    }
}

fn apply(op: Op, args: &[Value]) -> Value {
    let cmp = |want: fn(Ordering) -> bool| match num(&args[0]).compare(num(&args[1])) {
        Ok(o) => Value::Bool(want(o)),
        Err(e) => Value::Error(e.to_string().into()),
    };
    let fold = |f: fn(&Quantity, &Quantity) -> Result<Quantity, ValueError>| {
        let mut acc = num(&args[0]).clone();
        for a in &args[1..] {
            match f(&acc, num(a)) {
                Ok(q) => acc = q,
                Err(e) => return Value::Error(e.to_string().into()),
            }
        }
        Value::Num(acc)
    };
    match op {
        Op::Add => fold(Quantity::add),
        Op::Sub => fold(Quantity::sub),
        Op::Mul => fold(Quantity::mul),
        Op::Div => fold(Quantity::div),
        Op::Min => fold(Quantity::min),
        Op::Max => fold(Quantity::max),
        Op::Neg | Op::Abs => {
            let q = num(&args[0]);
            let n = if op == Op::Neg {
                q.num.checked_neg()
            } else {
                q.num.abs()
            };
            result(
                n.map(|num| Quantity {
                    num,
                    unit: q.unit.clone(),
                })
                .ok_or(ValueError::Overflow),
            )
        }
        Op::Round => result(num(&args[1]).round(num(&args[0]))),
        Op::Floor => result(num(&args[1]).floor(num(&args[0]))),
        Op::Ceil => result(num(&args[1]).ceil(num(&args[0]))),
        Op::Eq => Value::Bool(args[0] == args[1]),
        Op::Ne => Value::Bool(args[0] != args[1]),
        Op::Lt => cmp(Ordering::is_lt),
        Op::Le => cmp(Ordering::is_le),
        Op::Gt => cmp(Ordering::is_gt),
        Op::Ge => cmp(Ordering::is_ge),
    }
}

fn aggregate(
    agg: &Agg,
    collection: usize,
    body: Option<&Expr>,
    cx: &mut Context<'_, Value>,
    env: &mut Env,
) -> Value {
    if !cx.is_set(collection) {
        return Value::Missing;
    }
    let members = cx.members(collection);
    // Visit every member before giving up, so `unanswered` lists them all.
    let values: Vec<Value> = members
        .iter()
        .map(|&m| match body {
            Some(body) => {
                env.members.push((collection, m));
                let v = eval(body, cx, env);
                env.members.pop();
                v
            }
            None => Value::Bool(true),
        })
        .collect();
    if let Some(absent) = absent(&values) {
        // `any` and `all` can still be decided by a present member.
        let decided = match agg {
            Agg::Any => values.contains(&Value::Bool(true)),
            Agg::All => values.contains(&Value::Bool(false)),
            _ => false,
        };
        if !decided || matches!(absent, Value::Error(_)) {
            return absent;
        }
    }
    let truthy = values.iter().filter(|v| **v == Value::Bool(true)).count();
    match agg {
        Agg::Sum(zero) => {
            let mut acc = num(zero).clone();
            for v in &values {
                match acc.add(num(v)) {
                    Ok(q) => acc = q,
                    Err(e) => return Value::Error(e.to_string().into()),
                }
            }
            Value::Num(acc)
        }
        Agg::Count => Value::Num(Quantity::plain(crate::lisp::num::Num::int(truthy as i128))),
        Agg::Any => Value::Bool(truthy > 0),
        Agg::All => Value::Bool(!values.contains(&Value::Bool(false))),
        Agg::MinOf | Agg::MaxOf => {
            let op = if matches!(agg, Agg::MinOf) {
                Op::Min
            } else {
                Op::Max
            };
            if values.is_empty() {
                Value::Missing
            } else {
                apply(op, &values)
            }
        }
    }
}
