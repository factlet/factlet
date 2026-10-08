# factlet

An incremental fact graph for rules-as-code, with a small Lisp for writing
the graph as data.

You declare **inputs** (questions to ask), **definitions** (rules computed
from other facts) and **collections** (repeated groups such as W-2s). A
`Case` then holds one set of answers and computes on demand:

- **Incremental.** Changing an answer recomputes only the facts that depend
  on it, and stops early where a recomputed value comes out unchanged.
- **Missing-aware.** Unanswered inputs read as `?`, which propagates. Ask a
  case which questions block a fact with `unanswered`.
- **Explainable.** Every value comes with its derivation tree, annotated
  with labels, form lines and citations.
- **Forkable.** `fork` a case for a cheap what-if that shares everything
  already computed.
- **Exact.** Numbers are checked rationals with units: no floating point,
  no silent overflow, no adding dollars to percentages.

```lisp
(unit usd :prefix "$" :places 2)
(enum filing-status single married-joint)

(input filing-status : filing-status :label "Filing status")
(collection w2s :label "Forms W-2"
  (input wages : usd :line 1))

(def law/deduction :cite "IRC §63(c)(2)"
  (table filing-status
    single        $15,750
    married-joint $31,500))

(def taxable-income :label "Taxable income" :line 15
  (max $0 (- (sum w2s wages) law/deduction)))
```

```rust
use factlet::lisp::{Domain, load};

let program = load(SOURCE, &Domain::new())?;
let mut case = program.case();

program.set(&mut case, program.id("filing-status").unwrap(), "single")?;
let w2s = program.id("w2s").unwrap();
let acme = case.add_member(w2s, "acme")?;
program.set(&mut case, (program.id("w2s/*/wages").unwrap(), acme), "$58,000")?;

println!("{}", program.explain(&mut case, "taxable-income").unwrap());
```

```text
taxable-income = $42,250.00  ; Taxable income (line 15)
├── w2s = [#acme]  ; Forms W-2
├── w2s/#acme/wages = $58,000.00  ; line 1
└── law/deduction = $15,750.00  ; IRC §63(c)(2)
    └── filing-status = single  ; Filing status
```

See [examples/form1040](examples/form1040) for a slice of Form 1040 with an
interview, an explanation and a what-if:

```sh
cargo run --example form1040
```

## Command line

The `factlet` command checks, tests and runs programs without writing Rust.
It's behind the `cli` feature, so the library itself has no dependencies:

```sh
cargo install factlet --features cli

factlet check main.lisp                       # report every load error
factlet test main.lisp tests/                 # run (test …) forms (§7)
factlet eval main.lisp --case case.lisp --set 'interest=$100' taxable-income
factlet explain main.lisp --case case.lisp taxable-income
```

`eval` prints each fact's value, and for a missing one, the questions
blocking it. `--set` answers a global input as written in source; a
`--case` file answers anything with the scenario forms of §7. Includes are
read relative to the main file.

Units, enums and `brackets` are part of the language, so a tax program
needs nothing more. A program that needs functions written in Rust (§6)
can ship the same command, built on its domain:

```rust
fn main() -> std::process::ExitCode {
    factlet::cli::run(my_domain())
}
```

---

# Language specification

A program is a sequence of top-level **forms**. Forms may appear in any
order: a definition can read a fact declared below it.

## 1. Lexical syntax

| Syntax                     | Meaning                                                          |
| -------------------------- | ---------------------------------------------------------------- |
| `; …`                      | Comment to end of line                                           |
| `( … )`                    | List: a declaration or a call                                    |
| `[ … ]`                    | Vector: a list value, a type, parameters or `let` bindings       |
| `"text"`                   | String. Escapes: `\n` `\t` `\"` `\\`                             |
| `:name`                    | Keyword, for options such as `:label`                            |
| `'name`                    | Quoted symbol: an enum variant                                   |
| `2025-12-31`               | Date (`YYYY-MM-DD`; must be a real calendar date)                |
| `42` `-1.5` `$1,234.56` `22%` `5kWh` | Number literal (below)                                 |
| anything else              | Symbol                                                           |

Symbols are delimited by whitespace and `( ) [ ] " ; '`, so `-`,
`line16/tax`, `given?` and `!=` are all ordinary symbols.

### Number literals

```
number  = ["-"] [prefix] digits [suffix]
prefix  = symbol characters other than letters, digits and + - . , _ %   e.g. $ €
digits  = digit { digit | "_" | "," } [ "." digit { digit | "_" } ]
suffix  = any trailing characters, e.g. % or kWh
```

- `,` groups must be exactly three digits (`$1,234`, not `$12,34`).
- `22%` is the plain number `0.22`. A percentage can't also have a unit.
- A prefix or suffix names the unit that declared it (§3.1): with
  `(unit usd :prefix "$")`, `$5` is 5 usd. An unknown prefix or suffix is
  an error.
- Literals are exact: `0.1` is 1/10.

## 2. Types

| Type          | Values                                                           |
| ------------- | ---------------------------------------------------------------- |
| `number`      | A dimensionless exact rational: `3`, `0.5`, `22%`                |
| *unit name*   | A number in that unit, e.g. `usd`: `$1,234.56`                   |
| `bool`        | `true`, `false`                                                  |
| `string`      | `"text"`                                                         |
| `date`        | `2025-12-31`                                                     |
| *enum name*   | One of its variants, e.g. `'single`                              |
| `[t1 t2 …]`   | A fixed-shape list whose items have those types                  |
| `[t n]`       | Shorthand for a list of `n` items of type `t` (1 ≤ n ≤ 1000)     |

Every type also admits two absent values, which propagate through
expressions:

- **Missing** (printed `?`): an unanswered input, or anything computed
  from one.
- **Error** (printed `<error: …>`): a runtime failure such as division by
  zero, overflow, an index out of range, or a fixpoint that doesn't
  converge.

Programs are type-checked when they load. Types are inferred for
definitions; inputs and function parameters are annotated.

## 3. Declarations

### 3.1 `unit`

```lisp
(unit name [:prefix "str"] [:suffix "str"] [:places n])
```

Declares a unit of measure. `:prefix` and `:suffix` are how literals of the
unit are written and how its values print; `:places` (0–18, default 0) is
how many decimal places are shown. Values stay exact regardless of
`:places`.

```lisp
(unit usd :prefix "$" :places 2)   ; $1,234.50
(unit kwh :suffix "kWh")            ; 1,200kWh
```

A prefix or suffix may belong to only one unit, and the suffix `%` is
reserved for percentages.

### 3.2 `enum`

```lisp
(enum name variant …)
```

Declares an enumeration. Variant names must be unique across *all* enums in
the program, so a variant identifies its enum.

```lisp
(enum filing-status single married-joint married-separate head-of-household)
```

Units and enums are declared before everything else, so they may be used
anywhere in the program. They must be at the top level (they can appear
inside `module`, but are not prefixed by it).

### 3.3 `input`

```lisp
(input name : type [:default literal] [:key value …])
```

A question for the user. It reads Missing until answered, unless it has a
`:default`, which it reads instead (and which `Program::unanswered` then
doesn't report). The default must be a literal of the declared type.

```lisp
(input filing-status : filing-status :label "Filing status")
(input dependents : number :default 0)
(input birth-date : date)
(input monthly-premiums : [usd 12])
```

### 3.4 `def`

```lisp
(def name [:key value …] expression)
```

A fact computed from an expression (§4). Its type is the expression's type.
A definition whose expression reads no inputs is folded to a constant when
the program loads.

```lisp
(def law/std-deduction :cite "IRC §63(c)(2)" $15,750)
(def line11/agi :label "Adjusted gross income" :line 11
  (+ line1z/wages taxable-interest))
```

### 3.5 `collection`

```lisp
(collection name [:key value …] field …)
```

A repeated group. Each field is an `input` or a `def`; the case adds
members at run time (`Case::add_member`), and each member gets its own copy
of every field. Collections can't be nested.

```lisp
(collection w2s :label "Forms W-2"
  (input box1-wages : usd :line 1)
  (input box2-withheld : usd :line 2)
  (def withheld-rate (/ box2-withheld box1-wages)))
```

- Inside a field's `def`, the collection's other fields refer to the same
  member. Globals are visible too.
- Outside the collection, fields are only reachable through an aggregate
  (§4.7), such as `(sum w2s box1-wages)`. A collection's name alone is not
  an expression.
- From Rust, a field is named `collection/*/field` (`w2s/*/box1-wages`)
  and addressed as `(field_id, member)`.
- The member list is itself a question: until members are added, or
  `Case::set_empty` says there are none, aggregates over it are Missing.

### 3.6 `defn`

```lisp
(defn name [param : type …] body)
```

A function, called as `(name arg …)`. Arguments are checked against the
parameter types at each call site; the result type is inferred from the
body. The body can read its parameters and global facts, but not collection
fields (pass them in as arguments).

```lisp
(defn excess [a : usd b : usd] (max $0 (- a b)))
(def line34/refund (excess line25a/withholding line16/tax))
```

Functions can't be recursive (directly or mutually), can't be declared
inside a collection, and can't reuse the name of a built-in (§4) or a
domain function (§6). All arguments are evaluated before the call.

### 3.7 `fixpoint`

```lisp
(fixpoint name :start literal [:within tolerance] [:max rounds])
```

Definitions normally can't form a cycle. Some rules are genuinely circular
(the self-employed health insurance deduction depends on AGI, which depends
on the deduction), and `fixpoint` makes such a cycle legal by naming a
**break point** in it:

```lisp
(fixpoint sch1/line17 :start $0 :within $1)
(def sch1/line17 (min premiums-net se-profit))
(def premiums-net (excess premiums ptc))
(def line11/agi (- (+ wages se-profit) sch1/line17))
(def ptc (min premiums (excess benchmark (* 8.5% line11/agi))))
```

Evaluation is Gauss–Seidel iteration, as the IRS's iterative worksheets
run:

1. Each break point starts at its `:start` value.
2. Each round evaluates every member of the cycle in dependency order,
   each seeing the latest values; reads of a break point see the previous
   round's value.
3. The cycle has converged once no break point moved by more than its
   `:within` tolerance (or, without `:within`, didn't change at all).
   The other members are then recomputed from the settled break points.
4. After `:max` rounds (default 100, at most 10,000) without converging,
   every member is an Error.

Rules:

- `:start` and `:within` are literals of the break point's type.
- Every cycle must be cut into an acyclic order by its fixpoints; if not,
  add another `fixpoint`. A `fixpoint` naming a definition outside any
  cycle is an error.
- A cycle member can't be read from inside a `defn` body; pass it in as an
  argument.
- If any member reads Missing in a round, the whole cycle is Missing.

The cycle is computed by a hidden fact `fixpoint/<name>`, which also
exposes the number of rounds taken as `fixpoint/<name>/rounds`.

### 3.8 `module`

```lisp
(module name form …)
```

A namespace. Every name declared inside gets the prefix `name/`; modules
nest (`sch-a/limits/floor`).

Inside a module, a name is looked up in the module first, then each
enclosing module, then the top level. So inner names shadow outer ones,
and code outside reaches in with the full prefix:

```lisp
(def rate 10%)
(module state
  (def rate 5%)
  (defn tax [x : usd] (* x rate)))   ; state/rate
(def federal (* $100 rate))          ; rate
(def theirs (state/tax $1,000))      ; $50.00
```

A slash in a name is otherwise just a character: `(def law/rate 10%)` at
the top level is equivalent to `(module law (def rate 10%))`.

### 3.9 `include`

```lisp
(include "file")
```

Splices another file's forms in place, inside the current module. Files are
resolved by the loader passed to `load_files`, so they can come from disk,
memory or anywhere else. Including the same file twice into the same module
has no further effect; an include cycle is an error.

```lisp
(include "units.lisp")
(module law (include "law-2025.lisp"))   ; its names become law/…
```

### 3.10 Metadata

`input`, `def` and `collection` accept any `:key value` pairs before their
body, where the value is a string, number, symbol or date. They are kept
for tools, available from Rust as `Program::meta`. Three keys are shown by
`explain`:

| Key      | Use                         | Shown as                 |
| -------- | --------------------------- | ------------------------ |
| `:label` | Human-readable name         | `Tax`                    |
| `:line`  | Line on a form              | `(line 16)`              |
| `:cite`  | Legal authority             | `(…, IRC §1(j))`         |

## 4. Expressions

### 4.1 Atoms

| Expression           | Value                                                    |
| -------------------- | -------------------------------------------------------- |
| number, string, date | Itself                                                   |
| `true`, `false`      | Booleans                                                 |
| `'variant`           | An enum variant (a bare `variant` is not)                |
| `[e …]`              | A list of the values of `e …`; Missing if any is         |
| `name`               | A `let` binding or parameter, then a field of the        |
|                      | enclosing collection, then a global (§3.8 lookup)        |

### 4.2 Arithmetic

| Form                      | Notes                                                |
| ------------------------- | ---------------------------------------------------- |
| `(+ a b …)` `(- a b …)`   | Same unit throughout                                 |
| `(- a)`                   | Negation                                             |
| `(* a b …)`               | At most one operand may have a unit                  |
| `(/ a b …)`               | `usd / number` is `usd`; `usd / usd` is `number`     |
| `(min a b …)` `(max a b …)` | Same unit throughout                               |
| `(abs a)`                 |                                                      |
| `(round step x)`          | Nearest multiple of `step`, halves away from zero    |
| `(floor step x)` `(ceil step x)` | Toward −∞ / +∞                                |

`step` is in `x`'s unit or a plain number: `(round $1 tax)`,
`(floor 0.01 rate)`. Unit mismatches are reported when the program loads;
division by zero and overflow are runtime Errors.

`(brackets amount [rate edge rate edge … rate])` applies a progressive
schedule: each rate is charged on the part of `amount` up to the edge after
it, and the last rate on everything above the last edge. Rates are plain
numbers and edges are in `amount`'s unit, which is the result's:

```lisp
(def law/brackets/single [10% $11,925  12% $48,475  22% $103,350  24%])
(def tax (round $1 (brackets taxable-income law/brackets/single)))
; $42,250 → $1,192.50 + 12% × ($42,250 − $11,925) = $4,831.50 → $4,832
```

### 4.3 Comparison

| Form                                | Notes                                     |
| ----------------------------------- | ----------------------------------------- |
| `(= a b)` `(!= a b)`                | Any two values of the same type           |
| `(< a b)` `(<= a b)` `(> a b)` `(>= a b)` | Numbers of the same unit, or two dates |

### 4.4 Logic and conditionals

| Form                                | Notes                                         |
| ----------------------------------- | --------------------------------------------- |
| `(and a …)` `(or a …)` `(not a)`    | Three-valued (below)                          |
| `(if test then else)`               | Both branches have the same type              |
| `(cond (test value) … (else value))` | First true test wins; `else` is required and last |
| `(table key label value … )`        | Lookup by enum variant (below)                |
| `(let [name value …] body)`         | Sequential bindings: each sees the ones before |

`and` and `or` follow Kleene logic: `(and false ?)` is `false` and
`(or true ?)` is `true`, so an answer that decides the result doesn't wait
on the others. Otherwise a Missing operand makes the result Missing.

`table` maps each variant of an enum to a value. A label is a variant, a
list of variants, or `else`, and every variant must be covered exactly
once:

```lisp
(table filing-status
  (single married-separate)                   $15,750
  (married-joint qualifying-surviving-spouse) $31,500
  head-of-household                           $23,625)
```

### 4.5 Missing values

| Form               | Value                                                       |
| ------------------ | ----------------------------------------------------------- |
| `(given? x)`       | `true` if `x` is present, `false` if Missing; Errors pass through |
| `(or-else x y)`    | `x`, or `y` if `x` is Missing; Errors pass through          |

Everything else is strict: an operator, function or list with a Missing
argument is Missing, and one with an Error argument is that Error. Strict
forms still read every argument first, so `unanswered` reports every
blocking question at once, not just the first.

### 4.6 Dates

| Form                     | Value                                                     |
| ------------------------ | --------------------------------------------------------- |
| `(year d)` `(month d)` `(day d)` | Components, as numbers                            |
| `(age-on birth d)`       | Whole years, a year older on each birthday (Feb 29 birthdays turn on Feb 28) |
| `(months-between a b)`   | Whole months from `a` to `b`; negative if `b` is earlier  |
| `(days-between a b)`     | Days from `a` to `b`                                      |

### 4.7 Collections

Aggregates take a collection name and an expression evaluated once per
member, in which the collection's fields refer to that member:

| Form                    | Value                                              |
| ----------------------- | -------------------------------------------------- |
| `(sum c x)`             | Sum of `x`; zero for no members                    |
| `(count c)`             | Number of members                                  |
| `(count c test)`        | Number of members for which `test` is true         |
| `(any c test)` `(all c test)` | Three-valued, like `or` / `and`              |
| `(min-of c x)` `(max-of c x)` | Missing for no members                       |

```lisp
(def wages (sum w2s box1-wages))
(def big-w2s (count w2s (> box1-wages $100,000)))
```

### 4.8 Lists

| Form                    | Value                                                  |
| ----------------------- | ------------------------------------------------------ |
| `(nth xs k)`            | Item `k`, from 0. A literal `k` can index a list of mixed types; a computed one needs a list of one type |
| `(sum-list xs)`         | Sum of a list of numbers in one unit                   |
| `(map-list f xs …)`     | `f` (a `defn`) applied element-wise to equal-length lists |

```lisp
(input premiums : [usd 12])
(input slcsp : [usd 12])
(defn monthly-credit [paid : usd benchmark : usd] (min paid benchmark))
(def ptc (sum-list (map-list monthly-credit premiums slcsp)))
```

## 5. Diagnostics

A program is fully checked when it loads, and every error is reported at
the form that caused it, as `file:line:col: message`:

```text
main.lisp:2:11: unknown name `y`
a.lisp:2:8: `$2`: unknown unit prefix `$`
2:11: cycle: a -> b -> a; break it with (fixpoint a :start …)
```

Load-time errors include unknown names, functions and units; type and unit
mismatches; wrong arity; incomplete or duplicate `table` labels; names
defined twice; unbroken cycles; and include cycles. After one error, the
affected expression's type becomes unknown, so a single mistake isn't
reported again downstream.

## 6. Embedding

The `Domain` passed to `load` extends the language from Rust with units,
enums and functions. A domain function supplies a type check, run once at
load time, and an evaluator, which only ever sees present values:

```rust
let mut domain = Domain::new();
domain.unit(UnitDef { name: "usd".into(), prefix: Some("$".into()), suffix: None, places: 2 })?;
domain.builtin("slcsp", check_slcsp, eval_slcsp); // a premium looked up from a rate file
```

```lisp
(def benchmark (slcsp zip-code household-size))
```

The main entry points on `factlet::lisp::Program`:

| Method                     | Purpose                                                    |
| -------------------------- | ---------------------------------------------------------- |
| `load` / `load_files`      | Parse, check and compile a program                         |
| `case`                     | A new set of answers                                       |
| `id(name)`                 | A fact's id (`w2s/*/field` for fields)                     |
| `set(case, fact, "src")`   | Answer an input from source text, checking its type        |
| `get(case, name)`          | A fact's current value                                     |
| `unanswered(case, fact)`   | The questions blocking a fact                              |
| `explain(case, name)`      | A derivation tree, annotated with metadata                 |
| `meta(id)`                 | A fact's `:key value` metadata                             |

And on `Case`: `add_member`, `remove_member`, `set_empty`, `unset`, `fork`
and `stats`.

## 7. Tests

Test files hold `(test "name" form …)` forms, run against a program by
`factlet test` or `factlet::lisp::test::run_tests`:

```lisp
(test "single, one W-2"
  (expect line34/refund ?)
  (given filing-status 'single
         taxable-interest $0)
  (member w2s acme
    (given box1-wages $58,000 box2-withheld $6,000)
    (expect withheld-rate 10.34%))
  (empty dependents)
  (expect line15/taxable-income $42,250
          line16/tax $4,832))
```

| Form                          | Does                                                  |
| ----------------------------- | ----------------------------------------------------- |
| `(given name value …)`        | Answers inputs, checking their types                  |
| `(member c name form …)`      | Adds a member to collection `c`; inside, names are its fields |
| `(empty c …)`                 | Answers that a collection has no members              |
| `(expect name value …)`       | Checks facts' values; `?` expects Missing             |
| `(expect-error name …)`       | Checks that facts are Errors                          |

Each test starts from an unanswered case and runs its forms in order, so
it can expect, change an answer and expect again. Values are compared
exactly: `$42,250` matches `$42,250.00`. A failure is reported at the
expected value, as `tests/w2.lisp:9:30: line16/tax: expected $4,832.00, got
$4,831.00`.

A `--case` file for `eval` and `explain` (or
`factlet::lisp::test::scenario`) holds the same `given`, `member` and
`empty` forms, at the top level.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
