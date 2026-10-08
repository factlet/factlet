(test "single, one W-2"
  (expect taxable-income ?)
  (given filing-status 'single)
  (member w2s acme
    (given wages $58,000 withheld $5,800)
    (expect rate 10%))
  (expect taxable-income $42,250))

(test "no W-2s"
  (given filing-status 'married-joint)
  (empty w2s)
  (expect taxable-income $0))

(test "a W-2 with no wages"
  (given filing-status 'single)
  (member w2s zero
    (given wages $0 withheld $0)
    (expect-error rate)))
