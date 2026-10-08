use crate::case::Context;
use crate::graph::{Builder, Graph};
use crate::lisp::cycles;
use crate::lisp::domain::{Domain, Type};
use crate::lisp::eval::{Agg, Env, Expr, Fixpoint, Op, eval};
use crate::lisp::num::Num;
use crate::lisp::parser::{SExpr, SExprKind, Span, read_file};
use crate::lisp::value::{Quantity, UnitDef, Value, ValueError};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

/// An error in a program, at the form that caused it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    /// The file's name, or empty for a program loaded from one string.
    pub file: String,
    pub span: Span,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut at = self.file.clone();
        // Line 0 is the file as a whole, such as one that couldn't be found.
        if self.span.line > 0 {
            if !at.is_empty() {
                at.push(':');
            }
            at += &format!("{}:{}", self.span.line, self.span.col);
        }
        if at.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{at}: {}", self.message)
        }
    }
}

impl std::error::Error for Diagnostic {}

pub(crate) struct Compiled {
    pub graph: Arc<Graph<Value>>,
    pub domain: Domain,
    /// Each fact's type, by id; `None` for collections.
    pub types: Vec<Option<Type>>,
    /// Inputs declared with a `:default`, by id.
    pub defaulted: Vec<usize>,
}

enum DeclKind<'a> {
    /// A type, and the value read until answered.
    Input(Type, Value),
    Collection,
    Def(&'a SExpr),
}

struct Decl<'a> {
    name: &'a str,
    kind: DeclKind<'a>,
    /// The collection declaration this is a field of.
    scope: Option<usize>,
    /// The module it was declared in, which its rule's names resolve in.
    module: usize,
}

/// A `defn`: typed parameters and a body, checked once.
struct Func<'a> {
    module: usize,
    params: Vec<(&'a str, Type)>,
    body: &'a SExpr,
    state: FuncState,
}

enum FuncState {
    Todo,
    Visiting,
    Done(Arc<Expr>, Type),
}

/// `(fixpoint name :start value :within tolerance :max n)`.
struct FixDecl<'a> {
    name: &'a str,
    module: usize,
    /// The definition it names, once resolved.
    decl: Option<usize>,
    span: Span,
    start: &'a SExpr,
    within: Option<&'a SExpr>,
    max: u32,
}

/// A cycle of definitions, iterated as one rule.
struct Group {
    /// Definitions in evaluation order.
    members: Vec<usize>,
    /// Its `fixpoint`s.
    breaks: Vec<usize>,
}

/// The group being compiled: references to its members read guesses.
struct Active {
    slots: HashMap<usize, usize>,
    types: Vec<Option<Type>>,
}

/// Names a `defn` can't take.
const RESERVED: &[&str] = &[
    "+", "-", "*", "/", "min", "max", "abs", "round", "floor", "ceil", "=", "!=", "<", "<=", ">",
    ">=", "if", "cond", "and", "or", "not", "given?", "or-else", "let", "table", "sum", "count",
    "any", "all", "min-of", "max-of", "true", "false", "fixpoint",
];

#[derive(Clone, Copy, PartialEq)]
enum State {
    Todo,
    Visiting,
    Done(usize),
}

struct Compiler<'a> {
    domain: Domain,
    /// Each module's name prefixes to try, innermost first: `a/b/`, `a/`, ``.
    modules: Vec<Vec<String>>,
    decls: Vec<Decl<'a>>,
    globals: HashMap<&'a str, usize>,
    fields: HashMap<(usize, &'a str), usize>,
    funcs: HashMap<&'a str, Func<'a>>,
    fixpoints: Vec<FixDecl<'a>>,
    groups: Vec<Group>,
    /// Each declaration's group, if it's in a cycle.
    group_of: Vec<Option<usize>>,
    active: Option<Active>,
    /// In a cycle already reported as not broken enough.
    reported: Vec<bool>,
    state: Vec<State>,
    types: Vec<Type>,
    builder: Builder<Value>,
    /// Fact types by id, filled as facts are added.
    fact_types: Vec<Option<Type>>,
    defaulted: Vec<usize>,
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
    /// Compiling a member of the active cycle, whose references to other
    /// members read their current guesses.
    in_group: bool,
    module: usize,
}

fn sym(e: &SExpr) -> Option<&str> {
    match &e.kind {
        SExprKind::Symbol(s) => Some(s),
        _ => None,
    }
}

fn err<T>(span: Span, message: impl Into<String>) -> Result<T, Diagnostic> {
    Err(Diagnostic {
        file: String::new(),
        span,
        message: message.into(),
    })
}

pub(crate) fn compile(src: &str, domain: &Domain) -> Result<Compiled, Vec<Diagnostic>> {
    compile_files(
        "",
        &mut |name| name.is_empty().then(|| src.to_string()),
        domain,
    )
}

/// A form to declare, and the module it's in.
struct Item<'a> {
    form: &'a SExpr,
    module: usize,
}

/// Load `main` and every file it includes, then compile them as one
/// program.
pub(crate) fn compile_files(
    main: &str,
    load: &mut dyn FnMut(&str) -> Option<String>,
    domain: &Domain,
) -> Result<Compiled, Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let files = load_all(main, load, &mut errors);
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    let mut result = compile_loaded(&files, domain, errors);
    if let Err(errors) = &mut result {
        for e in errors {
            e.file = names.get(e.span.file as usize).unwrap_or(&main).to_string();
        }
    }
    result
}

/// Every file reachable through `(include "…")`, parsed, in load order.
fn load_all(
    main: &str,
    load: &mut dyn FnMut(&str) -> Option<String>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<(String, Vec<SExpr>)> {
    let mut files: Vec<(String, Vec<SExpr>)> = Vec::new();
    let mut queue = vec![(main.to_string(), Span::default())];
    let mut seen = HashSet::new();
    while let Some((name, from)) = queue.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(src) = load(&name) else {
            errors.push(Diagnostic {
                file: String::new(),
                span: from,
                message: format!("can't find `{name}`"),
            });
            continue;
        };
        let id = files.len() as u32;
        let forms = read_file(&src, id).unwrap_or_else(|e| {
            errors.push(Diagnostic {
                file: String::new(),
                span: e.span,
                message: e.message,
            });
            Vec::new()
        });
        let mut includes = Vec::new();
        find_includes(&forms, &mut includes);
        queue.extend(includes.into_iter().rev());
        files.push((name, forms));
    }
    files
}

fn find_includes(forms: &[SExpr], out: &mut Vec<(String, Span)>) {
    for form in forms {
        let SExprKind::List(items) = &form.kind else {
            continue;
        };
        match (items.first().and_then(sym), items.get(1).map(|e| &e.kind)) {
            (Some("include"), Some(SExprKind::Str(name))) => {
                out.push((name.to_string(), form.span))
            }
            (Some("module"), _) => find_includes(items.get(2..).unwrap_or_default(), out),
            _ => {}
        }
    }
}

/// Splice includes and modules into one list of declarations.
struct Flatten<'a> {
    files: &'a [(String, Vec<SExpr>)],
    index: HashMap<&'a str, usize>,
    /// Each module's prefix and parent.
    modules: Vec<(String, usize)>,
    items: Vec<Item<'a>>,
    /// Files being included, for cycle reports.
    stack: Vec<usize>,
    /// (file, module) pairs already spliced in.
    done: HashSet<(usize, usize)>,
    errors: Vec<Diagnostic>,
}

impl<'a> Flatten<'a> {
    fn forms(&mut self, forms: &'a [SExpr], module: usize) {
        for form in forms {
            let items = match &form.kind {
                SExprKind::List(items) => &items[..],
                _ => &[],
            };
            match items.first().and_then(sym) {
                Some("module") => {
                    let Some(name) = items.get(1).and_then(sym) else {
                        self.error(form.span, "expected (module name forms…)");
                        continue;
                    };
                    let prefix = format!("{}{name}/", self.modules[module].0);
                    self.modules.push((prefix, module));
                    let inner = self.modules.len() - 1;
                    self.forms(&items[2..], inner);
                }
                Some("include") => {
                    let name = match items {
                        [_, e] => match &e.kind {
                            SExprKind::Str(name) => name,
                            _ => {
                                self.error(form.span, "expected (include \"file\")");
                                continue;
                            }
                        },
                        _ => {
                            self.error(form.span, "expected (include \"file\")");
                            continue;
                        }
                    };
                    // A file that couldn't be loaded is already reported.
                    let Some(&file) = self.index.get(&**name) else {
                        continue;
                    };
                    if let Some(at) = self.stack.iter().position(|&f| f == file) {
                        let mut path: Vec<&str> = self.stack[at..]
                            .iter()
                            .map(|&f| self.files[f].0.as_str())
                            .collect();
                        path.push(&self.files[file].0);
                        let message = format!("include cycle: {}", path.join(" -> "));
                        self.error(form.span, message);
                        continue;
                    }
                    if !self.done.insert((file, module)) {
                        continue;
                    }
                    self.stack.push(file);
                    self.forms(&self.files[file].1, module);
                    self.stack.pop();
                }
                _ => self.items.push(Item { form, module }),
            }
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(Diagnostic {
            file: String::new(),
            span,
            message: message.into(),
        });
    }
}

fn compile_loaded(
    files: &[(String, Vec<SExpr>)],
    domain: &Domain,
    errors: Vec<Diagnostic>,
) -> Result<Compiled, Vec<Diagnostic>> {
    let mut flat = Flatten {
        files,
        index: files
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.as_str(), i))
            .collect(),
        modules: vec![(String::new(), 0)],
        items: Vec::new(),
        stack: vec![0],
        done: HashSet::from([(0, 0)]),
        errors,
    };
    if let Some((_, forms)) = files.first() {
        flat.forms(forms, 0);
    }
    let Flatten {
        modules,
        items,
        errors,
        ..
    } = flat;
    // Declared names with their module's prefix: `sch-a/line17`.
    let full: Vec<Option<String>> = items
        .iter()
        .map(|item| {
            let SExprKind::List(parts) = &item.form.kind else {
                return None;
            };
            match parts.first().and_then(sym) {
                Some("input" | "def" | "collection" | "defn") => {
                    let name = parts.get(1).and_then(sym)?;
                    Some(format!("{}{name}", modules[item.module].0))
                }
                _ => None,
            }
        })
        .collect();
    let prefixes = (0..modules.len())
        .map(|mut m| {
            let mut chain = vec![modules[m].0.clone()];
            while m != 0 {
                m = modules[m].1;
                chain.push(modules[m].0.clone());
            }
            chain
        })
        .collect();

    let mut c = Compiler {
        domain: domain.clone(),
        modules: prefixes,
        decls: Vec::new(),
        globals: HashMap::new(),
        fields: HashMap::new(),
        funcs: HashMap::new(),
        fixpoints: Vec::new(),
        groups: Vec::new(),
        group_of: Vec::new(),
        active: None,
        reported: Vec::new(),
        state: Vec::new(),
        types: Vec::new(),
        builder: Graph::builder(),
        fact_types: Vec::new(),
        defaulted: Vec::new(),
        stack: Vec::new(),
        errors,
    };
    // Types first, so declarations can use them in any order.
    for item in &items {
        if let Err(e) = c.declare_type(item.form) {
            c.errors.push(e);
        }
    }
    for (item, full) in items.iter().zip(&full) {
        if let Err(e) = c.declare(item.form, None, item.module, full.as_deref()) {
            c.errors.push(e);
        }
    }
    c.plan_cycles();
    // Inputs and collections in source order, then rules as needed.
    for i in 0..c.decls.len() {
        c.add_answer(i);
    }
    for i in 0..c.decls.len() {
        c.define(i);
    }
    // Check functions nothing calls, too.
    let mut names: Vec<&str> = c.funcs.keys().copied().collect();
    names.sort_unstable();
    for name in names {
        let _ = c.func_body(name, Span::default());
    }
    // Drop the placeholder errors of cycles already reported.
    c.errors.retain(|d| !d.message.is_empty());
    if !c.errors.is_empty() {
        c.errors.sort_by_key(|d| (d.span.file, d.span.start));
        return Err(c.errors);
    }
    let first = items.first().map_or(Span::default(), |i| i.form.span);
    let graph = c.builder.build().map_err(|errs| {
        errs.into_iter()
            .map(|e| Diagnostic {
                file: String::new(),
                span: first,
                message: e.to_string(),
            })
            .collect::<Vec<_>>()
    })?;
    Ok(Compiled {
        graph,
        domain: c.domain,
        types: c.fact_types,
        defaulted: c.defaulted,
    })
}

impl<'a> Compiler<'a> {
    /// A global declaration by name, as seen from `module`: its own names
    /// first, then each enclosing module's, then the top level's.
    fn global(&self, module: usize, s: &str) -> Option<usize> {
        self.modules[module]
            .iter()
            .find_map(|prefix| self.globals.get(format!("{prefix}{s}").as_str()).copied())
    }

    /// A function's full name, as seen from `module`.
    fn func(&self, module: usize, s: &str) -> Option<&'a str> {
        self.modules[module].iter().find_map(|prefix| {
            self.funcs
                .get_key_value(format!("{prefix}{s}").as_str())
                .map(|(k, _)| *k)
        })
    }

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

    /// Declare `form`, a top-level declaration named `full` (with its
    /// module's prefix), or a field of collection `scope`.
    fn declare(
        &mut self,
        form: &'a SExpr,
        scope: Option<usize>,
        module: usize,
        full: Option<&'a str>,
    ) -> Result<(), Diagnostic> {
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
                Some(e) if sym(e).is_some() => Ok((full.unwrap_or(sym(e).unwrap()), e.span)),
                Some(e) => err(e.span, format!("expected a name, found `{e}`")),
                None => err(span, "expected a name"),
            }
        };
        let (kind, name, name_span) = match head {
            Some("defn") if scope.is_none() => return self.declare_func(items, span, module, full),
            Some("fixpoint") if scope.is_none() => {
                return self.declare_fixpoint(items, span, module);
            }
            Some("fixpoint") => return err(span, "fixpoints must be declared at the top level"),
            Some("defn") => return err(span, "functions must be defined at the top level"),
            Some("input") => {
                let (name, name_span) = name_at(1)?;
                let (ty, default) = match items.get(2..) {
                    Some([colon, ty]) if sym(colon) == Some(":") => (ty, None),
                    Some([colon, ty, key, value])
                        if sym(colon) == Some(":")
                            && matches!(&key.kind, SExprKind::Keyword(k) if &**k == "default") =>
                    {
                        (ty, Some(value))
                    }
                    _ => return err(span, "expected (input name : type [:default value])"),
                };
                let ty = self.type_expr(ty)?;
                let default = match default {
                    None => Value::Missing,
                    Some(value) => {
                        let (v, t) = self.literal(value)?;
                        if !t.matches(&ty) {
                            return err(
                                value.span,
                                format!("`{name}` is {ty}, so its default can't be {t} `{value}`"),
                            );
                        }
                        v
                    }
                };
                (DeclKind::Input(ty, default), name, name_span)
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
                    format!(
                        "expected input, def, defn, collection, fixpoint, module, include, unit or enum, found `{form}`"
                    ),
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
        self.decls.push(Decl {
            name,
            kind,
            scope,
            module,
        });
        self.state.push(State::Todo);
        self.types.push(Type::Unknown);
        if is_collection {
            for field in &items[2..] {
                if let Err(e) = self.declare(field, Some(i), module, None) {
                    self.errors.push(e);
                }
            }
        }
        Ok(())
    }

    /// A type: `usd`, `number`, `bool`, `string`, an enum, or a list of
    /// types such as `[number usd number]`.
    fn type_expr(&self, e: &SExpr) -> Result<Type, Diagnostic> {
        match &e.kind {
            SExprKind::Symbol(t) => self.domain.ty(t).ok_or(()),
            SExprKind::Vector(items) => items
                .iter()
                .map(|t| self.type_expr(t))
                .collect::<Result<Vec<_>, _>>()
                .map(|ts| Type::List(ts.into()))
                .map_err(drop),
            _ => Err(()),
        }
        .or_else(|()| err(e.span, format!("unknown type `{e}`")))
    }

    /// `(defn name [param : type …] body)`.
    fn declare_func(
        &mut self,
        items: &'a [SExpr],
        span: Span,
        module: usize,
        full: Option<&'a str>,
    ) -> Result<(), Diagnostic> {
        let usage = "expected (defn name [param : type …] body)";
        let [_, name, params, body] = items else {
            return err(span, usage);
        };
        let (Some(name_str), SExprKind::Vector(params)) = (sym(name), &params.kind) else {
            return err(span, usage);
        };
        if RESERVED.contains(&name_str) || self.domain.get_builtin(name_str).is_some() {
            return err(name.span, format!("`{name_str}` is already a function"));
        }
        if params.len() % 3 != 0 {
            return err(span, usage);
        }
        let mut typed = Vec::new();
        for triple in params.chunks(3) {
            let (Some(p), Some(":")) = (sym(&triple[0]), sym(&triple[1])) else {
                return err(triple[0].span, "expected `param : type`");
            };
            if typed.iter().any(|(q, _)| *q == p) {
                return err(triple[0].span, format!("`{p}` is a parameter twice"));
            }
            typed.push((p, self.type_expr(&triple[2])?));
        }
        let func = Func {
            module,
            params: typed,
            body,
            state: FuncState::Todo,
        };
        let key = full.unwrap_or(name_str);
        if self.funcs.insert(key, func).is_some() {
            return err(name.span, format!("`{key}` is defined twice"));
        }
        Ok(())
    }

    /// A function's checked body and result type, checking it on first use.
    fn func_body(&mut self, name: &'a str, span: Span) -> Result<(Arc<Expr>, Type), Diagnostic> {
        let func = self.funcs.get_mut(name).expect("declared");
        match &func.state {
            FuncState::Done(body, ty) => return Ok((body.clone(), ty.clone())),
            FuncState::Visiting => return err(span, format!("recursive call to `{name}`")),
            FuncState::Todo => func.state = FuncState::Visiting,
        }
        let mut scope = Scope {
            vars: func.params.clone(),
            module: func.module,
            ..Scope::default()
        };
        let body = func.body;
        let (body, ty) = self.expr(body, &mut scope).unwrap_or_else(|e| {
            self.errors.push(e);
            (Expr::Lit(Value::Missing), Type::Unknown)
        });
        let body = Arc::new(body);
        let func = self.funcs.get_mut(name).expect("declared");
        func.state = FuncState::Done(body.clone(), ty.clone());
        Ok((body, ty))
    }

    fn apply(
        &mut self,
        name: &'a str,
        args: &'a [SExpr],
        span: Span,
        scope: &mut Scope<'a>,
    ) -> Result<(Expr, Type), Diagnostic> {
        let params = self.funcs[name].params.clone();
        if args.len() != params.len() {
            let n = params.len();
            return err(
                span,
                format!(
                    "`{name}` takes {n} argument{}",
                    if n == 1 { "" } else { "s" }
                ),
            );
        }
        let mut exprs = Vec::new();
        for (arg, (param, want)) in args.iter().zip(&params) {
            let (e, got) = self.expr(arg, scope)?;
            if !got.matches(want) {
                return err(
                    arg.span,
                    format!("`{name}` expects {want} for `{param}`, found {got} `{arg}`"),
                );
            }
            exprs.push(e);
        }
        let (body, ty) = self.func_body(name, span)?;
        Ok((Expr::Apply(body, exprs), ty))
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
            (DeclKind::Input(t, default), None) => {
                let id = self.builder.input(d.name, default.clone());
                (id, Some(t.clone()))
            }
            (DeclKind::Input(t, default), Some(c)) => {
                let State::Done(c) = self.state[c] else {
                    unreachable!("collections are added before their fields")
                };
                let id = self.builder.field_input(c, d.name, default.clone());
                (id, Some(t.clone()))
            }
            (DeclKind::Collection, _) => (self.builder.collection(d.name), None),
            (DeclKind::Def(_), _) => return,
        };
        if let DeclKind::Input(_, default) = &self.decls[i].kind
            && *default != Value::Missing
        {
            self.defaulted.push(id);
        }
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
        if let Some(g) = self.group_of[i] {
            self.define_group(g);
            return match self.state[i] {
                State::Done(id) => Some(id),
                _ => None,
            };
        }
        let DeclKind::Def(body) = self.decls[i].kind else {
            unreachable!("answers are added first")
        };
        self.state[i] = State::Visiting;
        self.stack.push(i);
        let mut scope = Scope {
            module: self.decls[i].module,
            ..Scope::default()
        };
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
        if let Some(i) = self.global(scope.module, s) {
            if scope.in_group
                && let Some(active) = &self.active
                && let Some(&slot) = active.slots.get(&i)
            {
                let ty = active.types[slot].clone().unwrap_or(Type::Unknown);
                return Ok((Expr::Guess(slot), ty));
            }
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
            let name = self.decls[i].name;
            if self.reported[i] {
                return err(span, "");
            }
            if self
                .active
                .as_ref()
                .is_some_and(|a| a.slots.contains_key(&i))
            {
                return err(
                    span,
                    format!("`{name}` is in a fixpoint cycle; pass it to functions as an argument"),
                );
            }
            return err(
                span,
                format!(
                    "cycle: {}; break it with (fixpoint {name} :start …)",
                    path.join(" -> ")
                ),
            );
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
            _ if self.func(scope.module, f).is_some() => {
                let name = self.func(scope.module, f).expect("checked");
                self.apply(name, args, span, scope)
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
            file: String::new(),
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
                    file: String::new(),
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
        let c = match sym(coll).and_then(|s| self.global(scope.module, s)) {
            Some(c) if matches!(self.decls[c].kind, DeclKind::Collection) => c,
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

fn rule(expr: Expr) -> impl Fn(&mut Context<'_, Value>) -> Value + Send + Sync + 'static {
    let expr = Arc::new(expr);
    move |cx| eval(&expr, cx, &mut Env::default())
}

/// What the cycle finder needs to avoid: names that aren't references.
#[derive(Default)]
struct Walk<'a> {
    module: usize,
    bound: Vec<&'a str>,
    collections: Vec<usize>,
    funcs: Vec<&'a str>,
    fields: Vec<usize>,
}

impl<'a> Compiler<'a> {
    fn declare_fixpoint(
        &mut self,
        items: &'a [SExpr],
        span: Span,
        module: usize,
    ) -> Result<(), Diagnostic> {
        let usage = "expected (fixpoint name :start value [:within tolerance] [:max rounds])";
        let Some(name) = items.get(1).and_then(sym) else {
            return err(span, usage);
        };
        let (mut start, mut within, mut max) = (None, None, 100);
        for pair in items[2..].chunks(2) {
            let [key, value] = pair else {
                return err(span, usage);
            };
            match (&key.kind, &value.kind) {
                (SExprKind::Keyword(k), _) if &**k == "start" => start = Some(value),
                (SExprKind::Keyword(k), _) if &**k == "within" => within = Some(value),
                (SExprKind::Keyword(k), SExprKind::Number(n))
                    if &**k == "max" && n.scale == 0 && (1..=10_000).contains(&n.digits) =>
                {
                    max = n.digits as u32
                }
                _ => return err(key.span, format!("bad fixpoint option `{key} {value}`")),
            }
        }
        let Some(start) = start else {
            return err(span, format!("fixpoint `{name}` needs a :start"));
        };
        if self
            .fixpoints
            .iter()
            .any(|f| f.name == name && f.module == module)
        {
            return err(items[1].span, format!("fixpoint `{name}` declared twice"));
        }
        self.fixpoints.push(FixDecl {
            name,
            module,
            decl: None,
            span: items[1].span,
            start,
            within,
            max,
        });
        Ok(())
    }

    /// Find the cycles among global definitions and check each is broken
    /// by its fixpoints.
    fn plan_cycles(&mut self) {
        let n = self.decls.len();
        self.group_of = vec![None; n];
        self.reported = vec![false; n];
        let mut edges = vec![Vec::new(); n];
        for f in 0..self.fixpoints.len() {
            let FixDecl { name, module, .. } = self.fixpoints[f];
            self.fixpoints[f].decl = self.global(module, name);
        }
        for (i, edge) in edges.iter_mut().enumerate() {
            if let (DeclKind::Def(body), None) = (&self.decls[i].kind, self.decls[i].scope) {
                let mut out = BTreeSet::new();
                let mut walk = Walk {
                    module: self.decls[i].module,
                    ..Walk::default()
                };
                self.refs(body, &mut walk, &mut out);
                *edge = out.into_iter().collect();
            }
        }
        let mut claimed = vec![false; self.fixpoints.len()];
        for component in cycles::cycles(&edges) {
            let breaks: Vec<usize> = (0..self.fixpoints.len())
                .filter(|&f| {
                    self.fixpoints[f]
                        .decl
                        .is_some_and(|d| component.contains(&d))
                })
                .collect();
            if breaks.is_empty() {
                continue; // reported as a plain cycle when compiled
            }
            for &b in &breaks {
                claimed[b] = true;
            }
            let cut: Vec<usize> = breaks
                .iter()
                .map(|&b| self.fixpoints[b].decl.expect("in the component"))
                .collect();
            let Some(members) = cycles::order(&component, &edges, &cut) else {
                let names: Vec<&str> = component.iter().map(|&d| self.decls[d].name).collect();
                for &d in &component {
                    self.reported[d] = true;
                }
                self.errors.push(Diagnostic {
                    file: String::new(),
                    span: self.fixpoints[breaks[0]].span,
                    message: format!(
                        "the cycle through {} needs more fixpoints to break it",
                        names.join(", ")
                    ),
                });
                continue;
            };
            for &m in &members {
                self.group_of[m] = Some(self.groups.len());
            }
            self.groups.push(Group { members, breaks });
        }
        for (f, claimed) in claimed.into_iter().enumerate() {
            if claimed {
                continue;
            }
            let FixDecl {
                name, span, decl, ..
            } = self.fixpoints[f];
            let message = match decl.map(|d| &self.decls[d].kind) {
                Some(DeclKind::Def(_)) => format!("`{name}` isn't in a cycle"),
                _ => format!("fixpoint `{name}` names no def"),
            };
            self.errors.push(Diagnostic {
                file: String::new(),
                span,
                message,
            });
        }
    }

    /// The global definitions `e` may read, directly or through the
    /// functions it calls and the field rules it aggregates.
    fn refs(&self, e: &'a SExpr, walk: &mut Walk<'a>, out: &mut BTreeSet<usize>) {
        match &e.kind {
            SExprKind::Symbol(s) => self.ref_name(s, walk, out),
            SExprKind::Vector(items) => {
                for item in items {
                    self.refs(item, walk, out);
                }
            }
            SExprKind::List(items) => {
                let Some((head, args)) = items.split_first() else {
                    return;
                };
                match sym(head) {
                    Some("let") => {
                        let depth = walk.bound.len();
                        if let Some(SExprKind::Vector(pairs)) = args.first().map(|a| &a.kind) {
                            for pair in pairs.chunks(2) {
                                if let [name, value] = pair {
                                    self.refs(value, walk, out);
                                    walk.bound.extend(sym(name));
                                }
                            }
                        }
                        for a in args.iter().skip(1) {
                            self.refs(a, walk, out);
                        }
                        walk.bound.truncate(depth);
                    }
                    Some("cond") => {
                        for arm in args {
                            if let SExprKind::List(parts) = &arm.kind {
                                for part in parts.iter().filter(|p| sym(p) != Some("else")) {
                                    self.refs(part, walk, out);
                                }
                            }
                        }
                    }
                    Some("table") => {
                        if let Some((key, arms)) = args.split_first() {
                            self.refs(key, walk, out);
                            for value in arms.chunks(2).filter_map(|p| p.get(1)) {
                                self.refs(value, walk, out);
                            }
                        }
                    }
                    Some("sum" | "count" | "any" | "all" | "min-of" | "max-of") => {
                        let c = args
                            .first()
                            .and_then(sym)
                            .and_then(|s| self.global(walk.module, s));
                        match c {
                            Some(c) if matches!(self.decls[c].kind, DeclKind::Collection) => {
                                walk.collections.push(c);
                                for a in &args[1..] {
                                    self.refs(a, walk, out);
                                }
                                walk.collections.pop();
                            }
                            _ => {
                                for a in args {
                                    self.refs(a, walk, out);
                                }
                            }
                        }
                    }
                    Some(f) => {
                        for a in args {
                            self.refs(a, walk, out);
                        }
                        if let Some(name) = self.func(walk.module, f)
                            && !walk.funcs.contains(&name)
                        {
                            let func = &self.funcs[name];
                            let mut inner = Walk {
                                module: func.module,
                                bound: func.params.iter().map(|(p, _)| *p).collect(),
                                funcs: std::mem::take(&mut walk.funcs),
                                fields: std::mem::take(&mut walk.fields),
                                ..Walk::default()
                            };
                            inner.funcs.push(name);
                            self.refs(func.body, &mut inner, out);
                            inner.funcs.pop();
                            walk.funcs = inner.funcs;
                            walk.fields = inner.fields;
                        }
                    }
                    None => {
                        for item in items {
                            self.refs(item, walk, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn ref_name(&self, s: &str, walk: &mut Walk<'a>, out: &mut BTreeSet<usize>) {
        if walk.bound.contains(&s) {
            return;
        }
        for &c in walk.collections.iter().rev() {
            if let Some(&f) = self.fields.get(&(c, s)) {
                if let DeclKind::Def(body) = self.decls[f].kind
                    && !walk.fields.contains(&f)
                {
                    let mut inner = Walk {
                        module: self.decls[f].module,
                        collections: vec![c],
                        funcs: std::mem::take(&mut walk.funcs),
                        fields: std::mem::take(&mut walk.fields),
                        ..Walk::default()
                    };
                    inner.fields.push(f);
                    self.refs(body, &mut inner, out);
                    inner.fields.pop();
                    walk.funcs = inner.funcs;
                    walk.fields = inner.fields;
                }
                return;
            }
        }
        if let Some(d) = self.global(walk.module, s)
            && matches!(self.decls[d].kind, DeclKind::Def(_))
        {
            out.insert(d);
        }
    }

    /// A literal value, for an input's default or a fixpoint option.
    fn literal(&mut self, e: &'a SExpr) -> Result<(Value, Type), Diagnostic> {
        let ok = matches!(
            &e.kind,
            SExprKind::Number(_) | SExprKind::Str(_) | SExprKind::Quote(_)
        ) || matches!(sym(e), Some("true" | "false"));
        if !ok {
            return err(e.span, format!("expected a literal, found `{e}`"));
        }
        match self.expr(e, &mut Scope::default())? {
            (Expr::Lit(v), t) => Ok((v, t)),
            _ => unreachable!("literals compile to literals"),
        }
    }

    /// Compile a cycle as one rule that iterates it, plus a fact per member
    /// that reads its converged value.
    fn define_group(&mut self, g: usize) {
        let members = self.groups[g].members.clone();
        let breaks = self.groups[g].breaks.clone();
        for &m in &members {
            self.state[m] = State::Visiting;
        }
        let slots: HashMap<usize, usize> =
            members.iter().enumerate().map(|(k, &m)| (m, k)).collect();
        let mut types = vec![None; members.len()];
        let mut starts = Vec::new();
        let mut max = 0;
        for &b in &breaks {
            let (decl, start, within) = {
                let f = &self.fixpoints[b];
                max = max.max(f.max);
                (f.decl.expect("resolved"), f.start, f.within)
            };
            let name = self.decls[decl].name;
            let slot = slots[&decl];
            let (start, ty) = match self.literal(start) {
                Ok(x) => x,
                Err(e) => {
                    self.errors.push(e);
                    (Value::Missing, Type::Unknown)
                }
            };
            let within = match within.map(|w| self.literal(w)) {
                None => None,
                Some(Ok((Value::Num(q), t))) if t.matches(&ty) => Some(q),
                Some(Ok((_, t))) => {
                    let span = within.unwrap().span;
                    self.errors.push(Diagnostic {
                        file: String::new(),
                        span,
                        message: format!("`{name}` is {ty}, so :within must be too, not {t}"),
                    });
                    None
                }
                Some(Err(e)) => {
                    self.errors.push(e);
                    None
                }
            };
            types[slot] = Some(ty);
            starts.push((slot, start, within));
        }

        let outer = self.active.replace(Active { slots, types });
        let mut order = Vec::new();
        for (slot, &m) in members.iter().enumerate() {
            let DeclKind::Def(body) = self.decls[m].kind else {
                unreachable!("cycles are of definitions")
            };
            self.stack.push(m);
            let mut scope = Scope {
                in_group: true,
                module: self.decls[m].module,
                ..Scope::default()
            };
            let (expr, ty) = self.expr(body, &mut scope).unwrap_or_else(|e| {
                self.errors.push(e);
                (Expr::Lit(Value::Missing), Type::Unknown)
            });
            self.stack.pop();
            let active = self.active.as_mut().expect("set above");
            match &active.types[slot] {
                Some(start) if !ty.matches(start) => {
                    let name = self.decls[m].name;
                    self.errors.push(Diagnostic {
                        file: String::new(),
                        span: body.span,
                        message: format!(
                            "fixpoint `{name}` starts as {start} but is computed as {ty}"
                        ),
                    });
                }
                Some(_) => {}
                None => active.types[slot] = Some(ty),
            }
            order.push(expr);
        }
        let active = std::mem::replace(&mut self.active, outer).expect("set above");
        let types: Vec<Type> = active
            .types
            .into_iter()
            .map(|t| t.unwrap_or(Type::Unknown))
            .collect();

        let first = self.decls[self.fixpoints[breaks[0]].decl.expect("resolved")].name;
        let fixpoint = Fixpoint {
            order,
            breaks: starts,
            max,
        };
        let hidden = self.builder.derived(
            &format!("fixpoint/{first}"),
            rule(Expr::Fixpoint(Arc::new(fixpoint))),
        );
        let mut list = types.clone();
        list.push(Type::Num(None));
        self.record(hidden, Some(Type::List(list.into())));
        for (slot, &m) in members.iter().enumerate() {
            let id = self
                .builder
                .derived(self.decls[m].name, rule(Expr::Nth(hidden, slot)));
            self.types[m] = types[slot].clone();
            self.state[m] = State::Done(id);
            self.record(id, Some(types[slot].clone()));
        }
        let rounds = self.builder.derived(
            &format!("fixpoint/{first}/rounds"),
            rule(Expr::Nth(hidden, members.len())),
        );
        self.record(rounds, Some(Type::Num(None)));
    }
}
