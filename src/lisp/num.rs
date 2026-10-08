use crate::lisp::parser::NumLit;
use std::cmp::Ordering;
use std::fmt;

/// An exact rational, always reduced with a positive denominator, so derived
/// equality is value equality. Arithmetic is checked: `None` on overflow or
/// division by zero, never a panic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Num {
    n: i128,
    d: i128,
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.unsigned_abs(), b.unsigned_abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1) as i128
}

impl Num {
    pub const ZERO: Num = Num { n: 0, d: 1 };
    pub const ONE: Num = Num { n: 1, d: 1 };

    pub fn int(n: i128) -> Num {
        Num { n, d: 1 }
    }

    pub fn ratio(n: i128, d: i128) -> Option<Num> {
        if d == 0 {
            return None;
        }
        let (n, d) = if d < 0 {
            (n.checked_neg()?, d.checked_neg()?)
        } else {
            (n, d)
        };
        let g = gcd(n, d);
        Some(Num { n: n / g, d: d / g })
    }

    /// `digits / 10^scale`, ignoring any prefix or suffix.
    pub fn from_lit(lit: &NumLit) -> Option<Num> {
        Num::ratio(lit.digits, 10i128.checked_pow(lit.scale)?)
    }

    pub fn numer(self) -> i128 {
        self.n
    }

    pub fn denom(self) -> i128 {
        self.d
    }

    pub fn is_zero(self) -> bool {
        self.n == 0
    }

    pub fn is_negative(self) -> bool {
        self.n < 0
    }

    pub fn checked_neg(self) -> Option<Num> {
        Some(Num {
            n: self.n.checked_neg()?,
            d: self.d,
        })
    }

    pub fn abs(self) -> Option<Num> {
        if self.n < 0 {
            self.checked_neg()
        } else {
            Some(self)
        }
    }

    pub fn checked_add(self, o: Num) -> Option<Num> {
        let g = gcd(self.d, o.d);
        let n = self
            .n
            .checked_mul(o.d / g)?
            .checked_add(o.n.checked_mul(self.d / g)?)?;
        Num::ratio(n, (self.d / g).checked_mul(o.d)?)
    }

    pub fn checked_sub(self, o: Num) -> Option<Num> {
        self.checked_add(o.checked_neg()?)
    }

    pub fn checked_mul(self, o: Num) -> Option<Num> {
        let (g1, g2) = (gcd(self.n, o.d), gcd(o.n, self.d));
        let n = (self.n / g1).checked_mul(o.n / g2)?;
        Num::ratio(n, (self.d / g2).checked_mul(o.d / g1)?)
    }

    pub fn checked_div(self, o: Num) -> Option<Num> {
        self.checked_mul(Num::ratio(o.d, o.n)?)
    }

    /// The nearest multiple of `step`, halves away from zero ($0.50 rounds
    /// up to $1, as the IRS rounds).
    pub fn round(self, step: Num) -> Option<Num> {
        self.to_multiple(step, |n, d| {
            let (q, r) = (n / d, n % d);
            if r.unsigned_abs() * 2 >= d.unsigned_abs() {
                q + n.signum()
            } else {
                q
            }
        })
    }

    pub fn floor(self, step: Num) -> Option<Num> {
        self.to_multiple(step, i128::div_euclid)
    }

    pub fn ceil(self, step: Num) -> Option<Num> {
        self.to_multiple(step, |n, d| -(-n).div_euclid(d))
    }

    pub fn trunc(self, step: Num) -> Option<Num> {
        self.to_multiple(step, |n, d| n / d)
    }

    fn to_multiple(self, step: Num, to_int: impl Fn(i128, i128) -> i128) -> Option<Num> {
        let step = step.abs()?;
        let q = self.checked_div(step)?;
        Num::int(to_int(q.n, q.d)).checked_mul(step)
    }

    /// A decimal rounded (halves away from zero) to `places`, ungrouped.
    pub fn fmt_fixed(self, places: u32) -> Option<String> {
        let scale = 10i128.checked_pow(places)?;
        let scaled = self
            .round(Num::ratio(1, scale)?)?
            .checked_mul(Num::int(scale))?
            .n;
        let digits = format!(
            "{:0>width$}",
            scaled.unsigned_abs(),
            width = places as usize + 1
        );
        let (int, frac) = digits.split_at(digits.len() - places as usize);
        let sign = if scaled < 0 { "-" } else { "" };
        Some(if frac.is_empty() {
            format!("{sign}{int}")
        } else {
            format!("{sign}{int}.{frac}")
        })
    }
}

impl Ord for Num {
    fn cmp(&self, o: &Num) -> Ordering {
        match (self.n.checked_mul(o.d), o.n.checked_mul(self.d)) {
            (Some(a), Some(b)) => a.cmp(&b),
            // Too large to cross-multiply; close enough to order.
            _ => (self.n as f64 / self.d as f64).total_cmp(&(o.n as f64 / o.d as f64)),
        }
    }
}

impl PartialOrd for Num {
    fn partial_cmp(&self, o: &Num) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Exact: a decimal when the denominator divides a power of ten (up to
/// 10^18), otherwise `n/d`.
impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let places = (0..=18).find(|&k| 10i128.pow(k) % self.d == 0);
        match places.and_then(|k| self.fmt_fixed(k)) {
            Some(s) => f.write_str(&s),
            None => write!(f, "{}/{}", self.n, self.d),
        }
    }
}
