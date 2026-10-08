; TY2025 scenarios for form1040.lisp, run by tests/form1040.rs, or with a
; binary built on factlet::cli and this example's domain.

(test "single filer, one W-2, then a side gig"
  (given filing-status 'single taxable-interest $0)
  (member w2s acme (given box1-wages $58,000 box2-withheld $6,000))
  (expect line15/taxable-income $42,250
          ; $1,192.50 + 12% x ($42,250 - $11,925) = $4,831.50
          line16/tax $4,832
          line34/refund $1,168
          line37/amount-owed $0)
  (member w2s side-gig (given box1-wages $4,000 box2-withheld $0))
  (expect line16/tax $5,312
          line34/refund $688))

(test "joint filers, two W-2s"
  (given filing-status 'married-joint taxable-interest $500)
  (member w2s acme (given box1-wages $70,000 box2-withheld $7,000))
  (member w2s globex (given box1-wages $50,000 box2-withheld $5,000))
  (expect line11/agi $120,500
          line15/taxable-income $89,000
          line16/tax $10,203
          line34/refund $1,797))

(test "head of household with nothing to tax"
  (given filing-status 'head-of-household taxable-interest $0)
  (empty w2s)
  (expect line16/tax $0))

(test "the refund waits on every answer"
  (expect line34/refund ?)
  (given filing-status 'single taxable-interest $0)
  (member w2s acme (given box1-wages $58,000))
  (expect line34/refund ?)
  (member w2s acme2 (given box1-wages $0 box2-withheld $0))
  (expect line34/refund ?))
