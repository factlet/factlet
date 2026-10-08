use crate::graph::{Builder, Graph};
use crate::lisp::domain::{Domain, Type};
use crate::lisp::eval::{Agg, Env, Expr, Op, eval};
use crate::lisp::num::Num;
use crate::lisp::parser::{SExpr, SExprKind, Span, read};
use crate::lisp::value::{Quantity, UnitDef, Value, ValueError};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// An error in a program, at the form that caused it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for Diagnostic {}

pub(crate) struct Compiled {
    pub graph: Arc<Graph<Value>>,
    pub domain: Domain,
    /// Each fact's type, by id; `None` for collections.
    pub types: Vec<Option<Type>>,
}

enum DeclKind<'a> {
    Input(Type),
    Collection,
    Def(&'a SExpr),
}

struct Decl<'a> {
    name: &'a str,
    kind: DeclKind<'a>,
    /// The collection declaration this is a field of.
    scope: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Todo,
    Visiting,
    Done(usize),
}

struct Compiler<'a> {
    domain: Domain,
    decls: Vec<Decl<'a>>,
    globals: HashMap<&'a str, usize>,
    fields: HashMap<(usize, &'a str), usize>,
    state: Vec<State>,
    types: Vec<Type>,
    builder: Builder<Value>,
    /// Fact types by id, filled as facts are added.
    fact_types: Vec<Option<Type>>,
    /// Definitions being compiled, for cycle reports.
    stack: Vec<usize>,
    errors: Vec<Diagnostic>,
}

/// Lexical scope while compiling one expression.
#[derive(Default)]
struct Scope<'a> {
    vars: Vec<(&'a str, Type)>,
    /// Collections whose fields are visible, innermost last: the field
    /// rule's own, then enclosing aggregates.
    collections: Vec<usize>,
}

fn sym(e: &SExpr) -> Option<&str> {
    match &e.kind {
        SExprKind::Symbol(s) => Some(s),
        _ => None,
    }
}

fn err<T>(span: Span, message: impl Into<String>) -> Result<T, Diagnostic> {
    Err(Diagnostic {
        span,
        message: message.into(),
    })
}

pub(crate) fn compile(src: &str, domain: &Domain) -> Result<Compiled, Vec<Diagnostic>> {
    let forms = read(src).map_err(|e| {
        vec![Diagnostic {
            span: e.span,
            message: e.message,
        }]
    })?;
    let mut c = Compiler {
        domain: domain.clone(),
        decls: Vec::new(),
        globals: HashMap::new(),
        fields: HashMap::new(),
        state: Vec::new(),
        types: Vec::new(),
        builder: Graph::builder(),
        fact_types: Vec::new(),
        stack: Vec::new(),
        errors: Vec::new(),
    };
    // Types first, so declarations can use them in any order.
    for form in &forms {
        if let Err(e) = c.declare_type(form) {
            c.errors.push(e);
        }
    }
    for form in &forms {
        if let Err(e) = c.declare(form, None) {
            c.errors.push(e);
        }
    }
    // Inputs and collections in source order, then rules as needed.
    for i in 0..c.decls.len() {
        c.add_answer(i);
    }
    for i in 0..c.decls.len() {
        c.define(i);
    }
    if !c.errors.is_empty() {
        c.errors.sort_by_key(|d| d.span.start);
        return Err(c.errors);
    }
    let graph = c.builder.build().map_err(|errs| {
        errs.into_iter()
            .map(|e| Diagnostic {
                span: forms[0].span,
                message: e.to_string(),
            })
            .collect::<Vec<_>>()
    })?;
    Ok(Compiled {
        graph,
        domain: c.domain,
        types: c.fact_types,
    })
}

impl<'a> Compiler<'a> {
    fn declare_type(&mut self, form: &SExpr) -> Result<(), Diagnostic> {
        let SExprKind::List(items) = &form.kind else {
            return Ok(());
        };
        let span = form.span;
        match items.first().and_then(sym) {
            Some("unit") => {
                let name = items.get(1).and_then(sym);
                let Some(name) = name else {
                    return err(span, "expected (unit name :prefix \"$\" :places 2)");
                };
                let mut def = UnitDef {
                    name: name.into(),
                    prefix: None,
                    suffix: None,
                    places: 0,
                };
                for pair in items[2..].chunks(2) {
                    let (key, value) = match pair {
                        [k, v] => (k, v),
                        [k] => return err(k.span, "missing value"),
                        _ => unreachable!(),
                    };
                    match (&key.kind, &value.kind) {
                        (SExprKind::Keyword(k), SExprKind::Str(s)) if &**k == "prefix" => {
                            def.prefix = Some(s.clone())
                        }
                        (SExprKind::Keyword(k), SExprKind::Str(s)) if &**k == "suffix" => {
                            def.suffix = Some(s.clone())
                        }
                        (SExprKind::Keyword(k), SExprKind::Number(n))
                            if &**k == "places" && n.scale == 0 && (0..=18).contains(&n.digits) =>
                        {
                            def.places = n.digits as u32
                        }
                        _ => return err(key.span, format!("bad unit option `{key} {value}`")),
                    }
                }
                self.domain.unit(def).map(drop).or_else(|e| err(span, e))
            }
            Some("enum") => {
                let mut names = Vec::new();
                for item in &items[1..] {
                    match sym(item) {
                        Some(s) => names.push(s),
                        None => return err(item.span, "expected a name"),
                    }
                }
                let Some((name, variants)) = names.split_first() else {
                    return err(span, "expected (enum name variant…)");
                };
                if variants.is_empty() {
                    return err(span, format!("enum `{name}` has no variants"));
                }
                self.domain
                    .enumeration(name, variants)
                    .map(drop)
                    .or_else(|e| err(span, e))
            }
            _ => Ok(()),
        }
    }

    fn declare(&mut self, form: &'a SExpr, scope: Option<usize>) -> Result<(), Diagnostic> {
        let span = form.span;
        let SExprKind::List(items) = &form.kind else {
            return err(span, format!("expected a declaration, found `{form}`"));
        };
        let head = items.first().and_then(sym);
        if matches!(head, Some("unit" | "enum")) {
            return match scope {
                None => Ok(()),
                Some(_) => err(span, "types must be declared at the top level"),
            };
        }
        let name_at = |i: usize| -> Result<(&'a str, Span), Diagnostic> {
            match items.get(i) {
                Some(e) if sym(e).is_some() => Ok((sym(e).unwrap(), e.span)),
                Some(e) => err(e.span, format!("expected a name, found `{e}`")),
                None => err(span, "expected a name"),
            }
        };
        let (kind, name, name_span) = match head {
            Some("input") => {
                let (name, name_span) = name_at(1)?;
                let ty = match items.get(2..) {
                    Some([colon, ty]) if sym(colon) == Some(":") => ty,
                    _ => return err(span, "expected (input name : type)"),
                };
                let Some(t) = sym(ty).and_then(|t| self.domain.ty(t)) else {
                    return err(ty.span, format!("unknown type `{ty}`"));
                };
                (DeclKind::Input(t), name, name_span)
            }
            Some("def") => {
                let (name, name_span) = name_at(1)?;
                match items.get(2..) {
                    Some([body]) => (DeclKind::Def(body), name, name_span),
                    _ => return err(span, "expected (def name expression)"),
                }
            }
            Some("collection") if scope.is_none() => {
                let (name, name_span) = name_at(1)?;
                (DeclKind::Collection, name, name_span)
            }
            Some("collection") => return err(span, "collections can't be nested"),
            _ => {
                return err(
                    span,
                    format!("expected input, def, collection, unit or enum, found `{form}`"),
                );
            }
        };
        let i = self.decls.len();
        let taken = match scope {
            Some(c) => self.fields.insert((c, name), i).is_some(),
            None => self.globals.insert(name, i).is_some(),
        };
        if taken {
            return err(name_span, format!("`{name}` is defined twice"));
        }
        let is_collection = matches!(kind, DeclKind::Collection);
        self.decls.push(Decl { name, kind, scope });
        self.state.push(State::Todo);
        self.types.push(Type::Unknown);
        if is_collection {
            for field in &items[2..] {
                if let Err(e) = self.declare(field, Some(i)) {
                    self.errors.push(e);
                }
            }
        }
        Ok(())
    }

    fn record(&mut self, id: usize, ty: Option<Type>) {
        if self.fact_types.len() <= id {
            self.fact_types.resize(id + 1, None);
        }
        self.fact_types[id] = ty;
    }

    fn add_answer(&mut self, i: usize) {
        let d = &self.decls[i];
        let (id, ty) = match (&d.kind, d.scope) {
            (DeclKind::Input(t), None) => {
                (self.builder.input(d.name, Value::Missing), Some(t.clone()))
            }
            (DeclKind::Input(t), Some(c)) => {
                let State::Done(c) = self.state[c] else {
                    unreachable!("collections are added before their fields")
                };
                (
                    self.builder.field_input(c, d.name, Value::Missing),
                    Some(t.clone()),
                )
            }
            (DeclKind::Collection, _) => (self.builder.collection(d.name), None),
            (DeclKind::Def(_), _) => return,
        };
        if let Some(t) = &ty {
            self.types[i] = t.clone();
        }
        self.state[i] = State::Done(id);
        self.record(id, ty);
    }

    /// Compile a definition and everything it reads, then add it.
    fn define(&mut self, i: usize) -> Option<usize> {
        match self.state[i] {
            State::Done(id) => return Some(id),
            State::Visiting => return None,
            State::Todo => {}
        }
        let DeclKind::Def(body) = self.decls[i].kind else {
            unreachable!("answers are added first")
        };
        self.state[i] = State::Visiting;
        self.stack.push(i);
        let mut scope = Scope::default();
        scope.collections.extend(self.decls[i].scope);
        let (expr, ty) = self.expr(body, &mut scope).unwrap_or_else(|e| {
            self.errors.push(e);
            (Expr::Lit(Value::Missing), Type::Unknown)
        });
        self.stack.pop();

        let name = self.decls[i].name;
        let expr = Arc::new(expr);
        let rule =
            move |cx: &mut crate::case::Context<'_, Value>| eval(&expr, cx, &mut Env::default());
        let id = match self.decls[i].scope {
            Some(c) => {
                let State::Done(c) = self.state[c] else {
                    unreachable!()
                };
                self.builder.field_derived(c, name, rule)
            }
            None => self.builder.derived(name, rule),
        };
        self.types[i] = ty.clone();
        self.state[i] = State::Done(id);
        self.record(id, Some(ty));
        Some(id)
    }

    fn expr(&mut self, e: &'a SExpr, scope: &mut Scope<'a>) -> Result<(Expr, Type), Diagnostic> {
        let span = e.span;
        match &e.kind {
            SExprKind::Number(lit) => {
                let q = self.domain.units.resolve(lit).or_else(|m| err(span, m))?;
                let ty = Type::Num(q.unit.clone());
                Ok((Expr::Lit(Value::Num(q)), ty))
            }
            SExprKind::Str(s) => Ok((Expr::Lit(Value::Str(s.as_ref().into())), Type::Str)),
            SExprKind::Quote(x) => {
                let v = sym(x)
                    .ok_or_else(|| format!("expected a variant, found `{x}`"))
                    .and_then(|name| self.domain.variant(name))
                    .or_else(|m| err(x.span, m))?;
                let ty = Type::Enum(v.ty().clone());
                Ok((Expr::Lit(Value::Enum(v)), ty))
            }
            SExprKind::Keyword(k) => err(span, format!("unexpected `:{k}`")),
            SExprKind::Vector(items) => {
                let (exprs, types) = self.exprs(items, scope)?;
                Ok((Expr::List(exprs), Type::List(types.into())))
            }
            SExprKind::Symbol(s) => self.name(s, span, scope),
            SExprKind::List(items) => {
                let Some((head, args)) = items.split_first() else {
                    return err(span, "empty form");
                };
                let Some(f) = sym(head) else {
                    return err(
                        head.span,
                        format!("expected a function name, found `{head}`"),
                    );
                };
                self.call(f, args, span, scope)
            }
        }
    }

    fn exprs(
        &mut self,
        items: &'a [SExpr],
        scope: &mut Scope<'a>,
    ) -> Result<(Vec<Expr>, Vec<Type>), Diagnostic> {
        let mut exprs = Vec::new();
        let mut types = Vec::new();
        for item in items {
            let (e, t) = self.expr(item, scope)?;
            exprs.push(e);
            types.push(t);
        }
        Ok((exprs, types))
    }

    fn name(
        &mut self,
        s: &'a str,
        span: Span,
        scope: &Scope<'a>,
    ) -> Result<(Expr, Type), Diagnostic> {
        match s {
            "true" => return Ok((Expr::Lit(Value::Bool(true)), Type::Bool)),
            "false" => return Ok((Expr::Lit(Value::Bool(false)), Type::Bool)),
            _ => {}
        }
        if let Some(i) = scope.vars.iter().rposition(|(v, _)| *v == s) {
            return Ok((Expr::Var(i), scope.vars[i].1.clone()));
        }
        for &c in scope.collections.iter().rev() {
            if let Some(&i) = self.fields.get(&(c, s)) {
                let (id, ty) = self.fact(i, span)?;
                let State::Done(collection) = self.state[c] else {
                    unreachable!()
                };
                return Ok((Expr::Field { id, collection }, ty));
            }
        }
        if let Some(&i) = self.globals.get(s) {
            if matches!(self.decls[i].kind, DeclKind::Collection) {
                return err(
                    span,
                    format!("`{s}` is a collection; use (sum {s} …), (count {s}) and so on"),
                );
            }
            let (id, ty) = self.fact(i, span)?;
            return Ok((Expr::Global(id), ty));
        }
        if let Some((c, _)) = self.fields.keys().find(|(_, f)| *f == s) {
            let c = self.decls[*c].name;
            return err(
                span,
                format!("`{s}` is a field of `{c}`; read it inside (sum {c} …) or a rule of `{c}`"),
            );
        }
        if self.domain.variant(s).is_ok() {
            return err(
                span,
                format!("unknown name `{s}`; for the variant, write '{s}"),
            );
        }
        err(span, format!("unknown name `{s}`"))
    }

    /// A declared fact's id and type, compiling it first if it's a rule.
    fn fact(&mut self, i: usize, span: Span) -> Result<(usize, Type), Diagnostic> {
        if self.state[i] == State::Visiting {
            let start = self.stack.iter().position(|&d| d == i).unwrap_or(0);
            let mut path: Vec<&str> = self.stack[start..]
                .iter()
                .map(|&d| self.decls[d].name)
                .collect();
            path.push(self.decls[i].name);
            return err(span, format!("cycle: {}", path.join(" -> ")));
        }
        match self.define(i) {
            Some(id) => Ok((id, self.types[i].clone())),
            None => err(span, format!("`{}` can't be computed", self.decls[i].name)),
        }
    }

    fn call(
        &mut self,
        f: &str,
        args: &'a [SExpr],
        span: Span,
        scope: &mut Scope<'a>,
    ) -> Result<(Expr, Type), Diagnostic> {
        let arity = |n: usize| -> Result<(), Diagnostic> {
            if args.len() == n {
                Ok(())
            } else {
                err(
                    span,
                    format!("`{f}` takes {n} argument{}", if n == 1 { "" } else { "s" }),
                )
            }
        };
        let op = match f {
            "+" => Some(Op::Add),
            "-" if args.len() == 1 => Some(Op::Neg),
            "-" => Some(Op::Sub),
            "*" => Some(Op::Mul),
            "/" => Some(Op::Div),
            "min" => Some(Op::Min),
            "max" => Some(Op::Max),
            "abs" => Some(Op::Abs),
            "round" => Some(Op::Round),
            "floor" => Some(Op::Floor),
            "ceil" => Some(Op::Ceil),
            "=" => Some(Op::Eq),
            "!=" => Some(Op::Ne),
            "<" => Some(Op::Lt),
            "<=" => Some(Op::Le),
            ">" => Some(Op::Gt),
            ">=" => Some(Op::Ge),
            _ => None,
        };
        if let Some(op) = op {
            let (exprs, types) = self.exprs(args, scope)?;
            let ty = self.op_type(op, f, &types, args, span)?;
            return Ok((Expr::Op(op, exprs), ty));
        }
        match f {
            "if" => {
                arity(3)?;
                let test = self.expect(&args[0], scope, &Type::Bool)?;
                let (then, t) = self.expr(&args[1], scope)?;
                let otherwise = self.expect(&args[2], scope, &t)?;
                Ok((Expr::If(Box::new([test, then, otherwise])), t))
            }
            "cond" => {
                let mut arms = Vec::new();
                let mut ty: Option<Type> = None;
                for (k, arm) in args.iter().enumerate() {
                    let SExprKind::List(pair) = &arm.kind else {
                        return err(arm.span, "expected (test value)");
                    };
                    let [test, value] = &pair[..] else {
                        return err(arm.span, "expected (test value)");
                    };
                    let (v, t) = match &ty {
                        Some(t) => (self.expect(value, scope, t)?, t.clone()),
                        None => self.expr(value, scope)?,
                    };
                    ty = Some(t);
                    if sym(test) == Some("else") {
                        if k + 1 != args.len() {
                            return err(arm.span, "`else` must be last");
                        }
                        return Ok((Expr::Cond(arms, Box::new(v)), ty.unwrap()));
                    }
                    arms.push((self.expect(test, scope, &Type::Bool)?, v));
                }
                err(span, "`cond` needs a final (else value)")
            }
            "and" | "or" | "not" => {
                if f == "not" {
                    arity(1)?;
                }
                let mut exprs = Vec::new();
                for a in args {
                    exprs.push(self.expect(a, scope, &Type::Bool)?);
                }
                let e = match f {
                    "and" => Expr::And(exprs),
                    "or" => Expr::Or(exprs),
                    _ => Expr::Not(Box::new(exprs.remove(0))),
                };
                Ok((e, Type::Bool))
            }
            "given?" => {
                arity(1)?;
                let (x, _) = self.expr(&args[0], scope)?;
                Ok((Expr::Given(Box::new(x)), Type::Bool))
            }
            "or-else" => {
                arity(2)?;
                let (x, t) = self.expr(&args[0], scope)?;
                let fallback = self.expect(&args[1], scope, &t)?;
                Ok((Expr::OrElse(Box::new([x, fallback])), t))
            }
            "let" => {
                let (Some(bindings), Some(body), 2) = (args.first(), args.get(1), args.len())
                else {
                    return err(span, "expected (let [name value …] body)");
                };
                let SExprKind::Vector(pairs) = &bindings.kind else {
                    return err(bindings.span, "expected [name value …]");
                };
                if pairs.len() % 2 != 0 {
                    return err(bindings.span, "expected [name value …]");
                }
                let depth = scope.vars.len();
                let mut values = Vec::new();
                for pair in pairs.chunks(2) {
                    let Some(name) = sym(&pair[0]) else {
                        return err(pair[0].span, "expected a name");
                    };
                    let (v, t) = self.expr(&pair[1], scope)?;
                    values.push(v);
                    scope.vars.push((name, t));
                }
                let body = self.expr(body, scope);
                scope.vars.truncate(depth);
                let (body, t) = body?;
                Ok((Expr::Let(values, Box::new(body)), t))
            }
            "table" => self.table(args, span, scope),
            "sum" | "count" | "any" | "all" | "min-of" | "max-of" => {
                self.aggregate(f, args, span, scope)
            }
            _ => {
                let Some(builtin) = self.domain.get_builtin(f).cloned() else {
                    return err(span, format!("unknown function `{f}`"));
                };
                let (exprs, types) = self.exprs(args, scope)?;
                if types.contains(&Type::Unknown) {
                    return Ok((Expr::Call(builtin.eval, exprs), Type::Unknown));
                }
                let ty = (builtin.check)(&types).or_else(|m| err(span, format!("`{f}`: {m}")))?;
                Ok((Expr::Call(builtin.eval, exprs), ty))
            }
        }
    }

    fn expect(
        &mut self,
        e: &'a SExpr,
        scope: &mut Scope<'a>,
        want: &Type,
    ) -> Result<Expr, Diagnostic> {
        let (x, t) = self.expr(e, scope)?;
        if !t.matches(want) {
            return err(e.span, format!("expected {want}, found {t} `{e}`"));
        }
        Ok(x)
    }

    /// The unit algebra of `Quantity`, checked on types.
    fn op_type(
        &self,
        op: Op,
        f: &str,
        types: &[Type],
        args: &[SExpr],
        span: Span,
    ) -> Result<Type, Diagnostic> {
        let (min, max) = match op {
            Op::Neg | Op::Abs => (1, 1),
            Op::Add | Op::Sub | Op::Mul | Op::Min | Op::Max => (1, usize::MAX),
            _ => (2, 2),
        };
        if types.len() < min || types.len() > max {
            let n = if min == max {
                format!("{min}")
            } else {
                format!("at least {min}")
            };
            return err(
                span,
                format!(
                    "`{f}` takes {n} argument{}",
                    if min == 1 && max == 1 { "" } else { "s" }
                ),
            );
        }
        if types.contains(&Type::Unknown) {
            return Ok(match op {
                Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => Type::Bool,
                _ => Type::Unknown,
            });
        }
        if matches!(op, Op::Eq | Op::Ne) {
            if types[0] != types[1] {
                return err(
                    span,
                    format!("`{f}` compares {} with {}", types[0], types[1]),
                );
            }
            return Ok(Type::Bool);
        }
        let mut units = Vec::new();
        for (t, a) in types.iter().zip(args) {
            match t {
                Type::Num(u) => units.push(Quantity {
                    num: Num::ONE,
                    unit: u.clone(),
                }),
                _ => return err(a.span, format!("`{f}` needs numbers, found {t} `{a}`")),
            }
        }
        let mismatch = |e: ValueError| Diagnostic {
            span,
            message: e.to_string(),
        };
        let fold = |f: fn(&Quantity, &Quantity) -> Result<Quantity, ValueError>| {
            let mut acc = units[0].clone();
            for u in &units[1..] {
                acc = f(&acc, u).map_err(mismatch)?;
            }
            Ok(Type::Num(acc.unit))
        };
        match op {
            Op::Add | Op::Sub | Op::Min | Op::Max => fold(Quantity::add),
            Op::Mul => fold(Quantity::mul),
            Op::Div => fold(Quantity::div),
            Op::Neg | Op::Abs => Ok(types[0].clone()),
            Op::Round | Op::Floor | Op::Ceil => {
                units[1].round(&units[0]).map_err(mismatch)?;
                Ok(types[1].clone())
            }
            Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                units[0].compare(&units[1]).map_err(mismatch)?;
                Ok(Type::Bool)
            }
            Op::Eq | Op::Ne => unreachable!(),
        }
    }

    /// `(table key label value … [else value])`, where a label is a variant
    /// or a list of them, and every variant is covered.
    fn table(
        &mut self,
        args: &'a [SExpr],
        span: Span,
        scope: &mut Scope<'a>,
    ) -> Result<(Expr, Type), Diagnostic> {
        let Some((key, arms)) = args.split_first() else {
            return err(span, "expected (table key label value …)");
        };
        let (key_expr, key_ty) = self.expr(key, scope)?;
        let def = match key_ty {
            Type::Enum(def) => def,
            Type::Unknown => return Ok((Expr::Lit(Value::Missing), Type::Unknown)),
            other => {
                return err(
                    key.span,
                    format!("`table` needs an enum key, found {other}"),
                );
            }
        };
        if arms.len() % 2 != 0 {
            return err(span, "`table` needs a value after each label");
        }
        let mut index: Vec<Option<usize>> = vec![None; def.variants.len()];
        let mut values = Vec::new();
        let mut ty: Option<Type> = None;
        let mut otherwise = None;
        for pair in arms.chunks(2) {
            let (label, value) = (&pair[0], &pair[1]);
            let labels: Vec<&SExpr> = match &label.kind {
                SExprKind::List(items) => items.iter().collect(),
                _ => vec![label],
            };
            let (v, t) = match &ty {
                Some(t) => (self.expect(value, scope, t)?, t.clone()),
                None => self.expr(value, scope)?,
            };
            ty = Some(t);
            values.push(v);
            let arm = values.len() - 1;
            for l in labels {
                let name = sym(l).ok_or_else(|| Diagnostic {
                    span: l.span,
                    message: format!("expected a variant of `{}`", def.name),
                })?;
                if name == "else" {
                    otherwise = Some(arm);
                    continue;
                }
                let Some(k) = def.variants.iter().position(|v| &**v == name) else {
                    return err(l.span, format!("`{name}` is not a `{}`", def.name));
                };
                if index[k].replace(arm).is_some() {
                    return err(l.span, format!("`{name}` is listed twice"));
                }
            }
        }
        let missing: Vec<&str> = def
            .variants
            .iter()
            .zip(&index)
            .filter(|(_, i)| i.is_none())
            .map(|(v, _)| &**v)
            .collect();
        if !missing.is_empty() && otherwise.is_none() {
            return err(
                span,
                format!("`table` doesn't cover {}", missing.join(", ")),
            );
        }
        let index = index
            .into_iter()
            .map(|i| i.or(otherwise).unwrap())
            .collect();
        let expr = Expr::Table {
            key: Box::new(key_expr),
            index,
            arms: values,
        };
        Ok((expr, ty.unwrap_or(Type::Unknown)))
    }

    fn aggregate(
        &mut self,
        f: &str,
        args: &'a [SExpr],
        span: Span,
        scope: &mut Scope<'a>,
    ) -> Result<(Expr, Type), Diagnostic> {
        let usage = || {
            format!(
                "expected ({f} collection{})",
                if f == "count" { " [test]" } else { " value" }
            )
        };
        let Some(coll) = args.first() else {
            return err(span, usage());
        };
        let c = match sym(coll).and_then(|s| self.globals.get(s)) {
            Some(&c) if matches!(self.decls[c].kind, DeclKind::Collection) => c,
            _ => return err(coll.span, format!("`{coll}` is not a collection")),
        };
        let State::Done(collection) = self.state[c] else {
            unreachable!()
        };
        let body = match (f, args.get(1..)) {
            ("count", Some([])) => None,
            (_, Some([body])) => Some(body),
            _ => return err(span, usage()),
        };
        let compiled = match body {
            Some(body) => {
                scope.collections.push(c);
                let r = self.expr(body, scope);
                scope.collections.pop();
                Some((r?, body))
            }
            None => None,
        };
        let (agg, ty) = match (f, &compiled) {
            ("count", Some(((_, t), b))) if !t.matches(&Type::Bool) => {
                return err(b.span, format!("expected bool, found {t} `{b}`"));
            }
            ("count", _) => (Agg::Count, Type::Num(None)),
            ("any" | "all", Some(((_, t), b))) => {
                if !t.matches(&Type::Bool) {
                    return err(b.span, format!("expected bool, found {t} `{b}`"));
                }
                (if f == "any" { Agg::Any } else { Agg::All }, Type::Bool)
            }
            (_, Some(((_, t), b))) => {
                let unit = match t {
                    Type::Num(u) => u.clone(),
                    Type::Unknown => None,
                    _ => return err(b.span, format!("`{f}` needs numbers, found {t} `{b}`")),
                };
                let agg = match f {
                    "sum" => Agg::Sum(Value::Num(Quantity {
                        num: Num::ZERO,
                        unit,
                    })),
                    "min-of" => Agg::MinOf,
                    _ => Agg::MaxOf,
                };
                (agg, t.clone())
            }
            _ => unreachable!(),
        };
        let body = compiled.map(|((e, _), _)| Box::new(e));
        Ok((
            Expr::Agg {
                agg,
                collection,
                body,
            },
            ty,
        ))
    }
}
