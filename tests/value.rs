use factlet::lisp::num::Num;
use factlet::lisp::parser::{SExprKind, read};
use factlet::lisp::value::{Quantity, UnitDef, Units, Value, ValueError};

fn units() -> Units {
    let mut units = Units::default();
    units
        .define(UnitDef {
            name: "usd".into(),
            prefix: Some("$".into()),
            suffix: None,
            places: 2,
        })
        .unwrap();
    units
        .define(UnitDef {
            name: "days".into(),
            prefix: None,
            suffix: Some("days".into()),
            places: 0,
        })
        .unwrap();
    units
}

fn resolve(units: &Units, src: &str) -> Result<Quantity, String> {
    match read(src).unwrap().remove(0).kind {
        SExprKind::Number(lit) => units.resolve(&lit),
        other => panic!("{src} read as {other:?}"),
    }
}

fn q(src: &str) -> Quantity {
    resolve(&units(), src).unwrap()
}

fn n(n: i128, d: i128) -> Num {
    Num::ratio(n, d).unwrap()
}

#[test]
fn numbers_are_exact() {
    assert_eq!(n(1, 3).checked_mul(Num::int(3)), Some(Num::ONE));
    assert_eq!(q("0.1").add(&q("0.2")).unwrap(), q("0.3"));
    assert_eq!(n(2, -4), n(-1, 2));
    assert_eq!(Num::ratio(1, 0), None);
    assert!(n(1, 3) < n(1, 2));
    assert_eq!(n(1, 3).to_string(), "1/3");
    assert_eq!(n(-1, 8).to_string(), "-0.125");
}

#[test]
fn tax_arithmetic() {
    assert_eq!(q("22%").mul(&q("$1,000.00")).unwrap(), q("$220"));
    assert_eq!(q("$1,234.50").round(&q("$1")).unwrap(), q("$1,235"));
    assert_eq!(q("-$0.50").round(&q("$1")).unwrap(), q("-$1"));
    assert_eq!(q("$0.49").round(&q("$1")).unwrap(), q("$0"));
    assert_eq!(q("$1,250").floor(&q("$100")).unwrap(), q("$1,200"));
    assert_eq!(q("$1,201").ceil(&q("$100")).unwrap(), q("$1,300"));
    assert_eq!(q("-$1,250").floor(&q("$100")).unwrap(), q("-$1,300"));
    assert_eq!(q("$10").round(&q("0.5")).unwrap(), q("$10"));
}

#[test]
fn unit_algebra() {
    let err = q("$5").add(&q("3days")).unwrap_err();
    assert_eq!(err.to_string(), "`+` can't combine usd and days");
    assert_eq!(
        q("$5").mul(&q("$2")).unwrap_err().to_string(),
        "`*` can't combine usd and usd"
    );
    assert_eq!(
        q("5").div(&q("$2")).unwrap_err().to_string(),
        "`/` can't combine a plain number and usd"
    );
    assert_eq!(q("$5").div(&q("$2")).unwrap(), q("2.5"));
    assert_eq!(q("$5").div(&q("2")).unwrap(), q("$2.50"));
    assert_eq!(q("$5").div(&q("$0")), Err(ValueError::DivByZero));
    assert_eq!(q("$5").max(&q("$7")).unwrap(), q("$7"));
    assert!(q("$5").min(&q("5")).is_err());
    assert!(q("3days").round(&q("$1")).is_err());
}

#[test]
fn literals_resolve_through_units() {
    let units = units();
    let money = q("$1,234.56");
    assert_eq!(money.num, n(123_456, 100));
    assert_eq!(money.unit, units.get("usd").cloned());
    assert_eq!(q("7.65%"), Quantity::plain(n(765, 10_000)));
    assert_eq!(q("5days").unit, units.get("days").cloned());

    assert_eq!(
        resolve(&units, "£5").unwrap_err(),
        "`£5`: unknown unit prefix `£`"
    );
    assert_eq!(
        resolve(&units, "5kWh").unwrap_err(),
        "`5kWh`: unknown unit suffix `kWh`"
    );
    assert_eq!(
        resolve(&units, "$5%").unwrap_err(),
        "`$5%`: a percentage can't have a unit"
    );

    let mut units = units;
    let dup = UnitDef {
        name: "cad".into(),
        prefix: Some("$".into()),
        suffix: None,
        places: 2,
    };
    assert_eq!(units.define(dup).unwrap_err(), "`$` already means `usd`");
}

#[test]
fn display() {
    assert_eq!(q("$58000").to_string(), "$58,000.00");
    assert_eq!(q("-$5").to_string(), "-$5.00");
    assert_eq!(q("$0.005").to_string(), "$0.01");
    assert_eq!(q("$999.999").to_string(), "$1,000.00");
    assert_eq!(q("1500days").to_string(), "1,500days");
    assert_eq!(q("22%").to_string(), "0.22");
    assert_eq!(Value::Num(q("$1")).to_string(), "$1.00");
    assert_eq!(Value::Missing.to_string(), "?");
}

#[test]
fn overflow_is_an_error() {
    let big = Quantity::plain(Num::int(i128::MAX / 2));
    assert_eq!(big.mul(&big), Err(ValueError::Overflow));
    assert_eq!(big.add(&big).map(|_| ()), Ok(()));
    assert_eq!(big.add(&big).unwrap().add(&big), Err(ValueError::Overflow));
}
