//! Interview -> explain -> what-if on a Form 1040 written in factlet's Lisp.
//!
//!     cargo run --example form1040

mod tax;

use factlet::lisp::load;

const SOURCE: &str = include_str!("form1040.lisp");

fn main() {
    let program = match load(SOURCE, &tax::domain()) {
        Ok(p) => p,
        Err(errors) => {
            for e in errors {
                eprintln!("form1040.lisp:{e}");
            }
            std::process::exit(1);
        }
    };
    let id = |name: &str| program.id(name).unwrap();
    let (refund, tax) = (id("line34/refund"), id("line16/tax"));
    let mut c = program.case();

    println!("== Before any answers, the refund is blocked on:");
    for fact in c.unanswered(refund) {
        println!("   {}", c.name(fact));
    }

    let w2s = id("w2s");
    let (box1, box2) = (id("w2s/*/box1-wages"), id("w2s/*/box2-withheld"));
    program.set(&mut c, id("filing-status"), "single").unwrap();
    let acme = c.add_member(w2s, "acme").unwrap();
    program.set(&mut c, (box1, acme), "$58,000").unwrap();
    program.set(&mut c, (box2, acme), "$6,000").unwrap();
    program.set(&mut c, id("taxable-interest"), "$0").unwrap();

    println!("\n== Answered:\n{}", c.explain(refund));

    c.reset_stats();
    let side = c.add_member(w2s, "side-gig").unwrap();
    program.set(&mut c, (box1, side), "$4,000").unwrap();
    program.set(&mut c, (box2, side), "$0").unwrap();
    println!("== Second W-2: refund {}  {:?}", c.get(refund), c.stats());

    c.reset_stats();
    program.set(&mut c, id("taxable-interest"), "$12").unwrap();
    let owed = c.get(id("line37/amount-owed"));
    println!(
        "== $12 of interest: refund {}, owed {owed}  {:?}",
        c.get(refund),
        c.stats()
    );

    let mut what_if = c.fork();
    what_if.reset_stats();
    program
        .set(&mut what_if, id("filing-status"), "head-of-household")
        .unwrap();
    println!(
        "\n== What if head of household? tax {} (vs {})  {:?}",
        what_if.get(tax),
        c.get(tax),
        what_if.stats()
    );
}
