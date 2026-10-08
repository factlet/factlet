use factlet::lisp::parser::{NumLit, SExprKind, read};

fn round_trip(src: &str) -> String {
    let forms = read(src).unwrap();
    forms
        .iter()
        .map(|f| f.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn number(src: &str) -> NumLit {
    match read(src).unwrap().remove(0).kind {
        SExprKind::Number(n) => n,
        other => panic!("{src} read as {other:?}"),
    }
}

fn error(src: &str) -> String {
    read(src).unwrap_err().to_string()
}

#[test]
fn reads_forms() {
    let src = r#"
        ; law
        (def line15/taxable-income
          (max $0 (- line11/agi std-deduction)))  ; clamp
        [10% $11,925] 'single :prefix "a \"q\"\n" :
    "#;
    assert_eq!(
        round_trip(src),
        r#"(def line15/taxable-income (max $0 (- line11/agi std-deduction))) [10% $11925] 'single :prefix "a \"q\"\n" :"#
    );
}

#[test]
fn spans_point_at_forms() {
    let forms = read("(a\n  (b c))").unwrap();
    let SExprKind::List(items) = &forms[0].kind else {
        panic!()
    };
    let b = &items[1];
    assert_eq!((b.span.line, b.span.col), (2, 3));
    assert_eq!((b.span.start, b.span.end), (5, 10));
}

#[test]
fn reads_numbers() {
    let n = number("-$1,234.56");
    assert_eq!((n.digits, n.scale), (-123_456, 2));
    assert_eq!(n.prefix.as_deref(), Some("$"));
    assert_eq!(n.suffix, None);

    let n = number("7.65%");
    assert_eq!(
        (n.digits, n.scale, n.suffix.as_deref()),
        (765, 2, Some("%"))
    );
    assert_eq!(number("1_000kWh").suffix.as_deref(), Some("kWh"));
    assert_eq!(number("€0.5").to_string(), "€0.5");
    assert_eq!(number("$0.05").to_string(), "$0.05");

    for symbol in ["-", "+", "-x", "a1", "line1z/wages", "$", "..."] {
        let kind = &read(symbol).unwrap()[0].kind;
        assert!(
            matches!(kind, SExprKind::Symbol(_)),
            "{symbol} read as {kind:?}"
        );
    }
}

#[test]
fn reports_errors_with_positions() {
    assert_eq!(error("(a (b c)"), "1:1: unclosed, expected `)`");
    assert_eq!(error("\n  (a]"), "2:5: expected `)`, found `]`");
    assert_eq!(error("a)"), "1:2: unexpected `)`");
    assert_eq!(error("(x \"abc"), "1:4: unterminated string");
    assert_eq!(error("\"\\q\""), "1:2: unknown escape");
    assert_eq!(
        error("$1,23"),
        "1:1: bad number `$1,23`: `,` must separate groups of three digits"
    );
    assert_eq!(
        error("1.2.3"),
        "1:1: bad number `1.2.3`: misplaced `.` or `,`"
    );
    assert_eq!(error("'"), "1:1: nothing to quote");
}
