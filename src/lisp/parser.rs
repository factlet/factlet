use crate::lisp::date::Date;
use std::fmt;

/// A byte range in a source file, with the 1-based line and column it
/// starts at.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub col: u32,
    /// Which file, when a program has several; 0 otherwise.
    pub file: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct SExpr {
    pub kind: SExprKind,
    pub span: Span,
}

#[derive(Clone, PartialEq, Debug)]
pub enum SExprKind {
    List(Vec<SExpr>),
    /// `[a b c]`
    Vector(Vec<SExpr>),
    Symbol(Box<str>),
    /// `:prefix`, stored without the colon.
    Keyword(Box<str>),
    Str(Box<str>),
    Number(NumLit),
    /// `2025-12-31`
    Date(Date),
    /// `'x`
    Quote(Box<SExpr>),
}

/// A number as written: `-$1,234.56` is `-123456` at scale 2 with prefix
/// `$`. What a prefix or suffix means is up to the domain.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NumLit {
    pub digits: i128,
    /// Digits after the decimal point.
    pub scale: u32,
    pub prefix: Option<Box<str>>,
    /// `%`, or a unit name such as `kWh`.
    pub suffix: Option<Box<str>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReadError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for ReadError {}

/// Read every form in `src`.
pub fn read(src: &str) -> Result<Vec<SExpr>, ReadError> {
    read_file(src, 0)
}

/// Read every form in `src`, marking spans as from `file`.
pub fn read_file(src: &str, file: u32) -> Result<Vec<SExpr>, ReadError> {
    let mut r = Parser {
        src,
        pos: 0,
        line: 1,
        col: 1,
        file,
    };
    let mut forms = Vec::new();
    while r.skip_space() {
        forms.push(r.form()?);
    }
    Ok(forms)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    line: u32,
    col: u32,
    file: u32,
}

const DELIMS: &str = "()[]\";'";

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    /// Skip whitespace and comments; false at end of input.
    fn skip_space(&mut self) -> bool {
        while let Some(c) = self.peek() {
            if c == ';' {
                while self.peek().is_some_and(|c| c != '\n') {
                    self.bump();
                }
            } else if c.is_whitespace() {
                self.bump();
            } else {
                return true;
            }
        }
        false
    }

    fn here(&self) -> Span {
        Span {
            start: self.pos,
            end: self.pos,
            line: self.line,
            col: self.col,
            file: self.file,
        }
    }

    fn error<T>(&self, span: Span, message: impl Into<String>) -> Result<T, ReadError> {
        Err(ReadError {
            span,
            message: message.into(),
        })
    }

    fn form(&mut self) -> Result<SExpr, ReadError> {
        let mut span = self.here();
        let kind = match self.peek().expect("skip_space found a form") {
            '(' => SExprKind::List(self.seq(')')?),
            '[' => SExprKind::Vector(self.seq(']')?),
            c @ (')' | ']') => return self.error(span, format!("unexpected `{c}`")),
            '\'' => {
                self.bump();
                if !self.skip_space() {
                    return self.error(span, "nothing to quote");
                }
                SExprKind::Quote(Box::new(self.form()?))
            }
            '"' => SExprKind::Str(self.string()?),
            _ => self.atom()?,
        };
        span.end = self.pos;
        Ok(SExpr { kind, span })
    }

    fn seq(&mut self, close: char) -> Result<Vec<SExpr>, ReadError> {
        let open = self.here();
        self.bump();
        let mut items = Vec::new();
        loop {
            if !self.skip_space() {
                return self.error(open, format!("unclosed, expected `{close}`"));
            }
            match self.peek() {
                Some(c) if c == close => {
                    self.bump();
                    return Ok(items);
                }
                Some(c @ (')' | ']')) => {
                    return self.error(self.here(), format!("expected `{close}`, found `{c}`"));
                }
                _ => items.push(self.form()?),
            }
        }
    }

    fn string(&mut self) -> Result<Box<str>, ReadError> {
        let open = self.here();
        self.bump();
        let mut out = String::new();
        loop {
            let at = self.here();
            match self.bump() {
                None => return self.error(open, "unterminated string"),
                Some('"') => return Ok(out.into()),
                Some('\\') => match self.bump() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(c @ ('"' | '\\')) => out.push(c),
                    _ => return self.error(at, "unknown escape"),
                },
                Some(c) => out.push(c),
            }
        }
    }

    fn atom(&mut self) -> Result<SExprKind, ReadError> {
        let span = self.here();
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| !c.is_whitespace() && !DELIMS.contains(c))
        {
            self.bump();
        }
        let text = &self.src[start..self.pos];
        if let Some(kw) = text.strip_prefix(':')
            && !kw.is_empty()
        {
            return Ok(SExprKind::Keyword(kw.into()));
        }
        match Date::parse(text) {
            Some(Ok(d)) => return Ok(SExprKind::Date(d)),
            Some(Err(message)) => return self.error(span, message),
            None => {}
        }
        match number(text) {
            Some(Ok(n)) => Ok(SExprKind::Number(n)),
            Some(Err(message)) => self.error(span, format!("bad number `{text}`: {message}")),
            None => Ok(SExprKind::Symbol(text.into())),
        }
    }
}

/// `None` if `text` isn't shaped like a number: an optional `-`, an optional
/// prefix of symbol characters such as `$`, then a digit.
fn number(text: &str) -> Option<Result<NumLit, &'static str>> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let body_at = rest.find(|c: char| c.is_ascii_digit())?;
    let prefix = &rest[..body_at];
    if prefix
        .chars()
        .any(|c| c.is_alphanumeric() || "+-.,_%".contains(c))
    {
        return None;
    }
    let rest = &rest[body_at..];
    let body_len = rest
        .find(|c: char| !(c.is_ascii_digit() || ",._".contains(c)))
        .unwrap_or(rest.len());
    let (body, suffix) = rest.split_at(body_len);
    Some(parse_body(body).map(|(digits, scale)| NumLit {
        digits: if negative { -digits } else { digits },
        scale,
        prefix: (!prefix.is_empty()).then(|| prefix.into()),
        suffix: (!suffix.is_empty()).then(|| suffix.into()),
    }))
}

fn parse_body(body: &str) -> Result<(i128, u32), &'static str> {
    let (int, frac) = match body.split_once('.') {
        Some((int, frac)) => (int, frac),
        None => (body, ""),
    };
    if body.ends_with('.') || frac.contains(['.', ',']) {
        return Err("misplaced `.` or `,`");
    }
    if int.contains(',') {
        let mut groups = int.split(',');
        let first = groups.next().unwrap_or_default();
        if first.is_empty() || first.len() > 3 || groups.any(|g| g.len() != 3) {
            return Err("`,` must separate groups of three digits");
        }
    }
    let mut digits: i128 = 0;
    for c in int.chars().chain(frac.chars()).filter(char::is_ascii_digit) {
        digits = digits
            .checked_mul(10)
            .and_then(|d| d.checked_add(i128::from(c as u8 - b'0')))
            .ok_or("too large")?;
    }
    Ok((
        digits,
        frac.chars().filter(char::is_ascii_digit).count() as u32,
    ))
}

impl fmt::Display for SExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seq = |f: &mut fmt::Formatter<'_>, items: &[SExpr], open, close| {
            f.write_str(open)?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    f.write_str(" ")?;
                }
                write!(f, "{item}")?;
            }
            f.write_str(close)
        };
        match &self.kind {
            SExprKind::List(items) => seq(f, items, "(", ")"),
            SExprKind::Vector(items) => seq(f, items, "[", "]"),
            SExprKind::Symbol(s) => f.write_str(s),
            SExprKind::Keyword(k) => write!(f, ":{k}"),
            SExprKind::Str(s) => write!(f, "{s:?}"),
            SExprKind::Number(n) => write!(f, "{n}"),
            SExprKind::Date(d) => write!(f, "{d}"),
            SExprKind::Quote(x) => write!(f, "'{x}"),
        }
    }
}

impl fmt::Display for NumLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.digits < 0 {
            f.write_str("-")?;
        }
        f.write_str(self.prefix.as_deref().unwrap_or_default())?;
        let digits = format!(
            "{:0>width$}",
            self.digits.unsigned_abs(),
            width = self.scale as usize + 1
        );
        let (int, frac) = digits.split_at(digits.len() - self.scale as usize);
        f.write_str(int)?;
        if !frac.is_empty() {
            write!(f, ".{frac}")?;
        }
        f.write_str(self.suffix.as_deref().unwrap_or_default())
    }
}
