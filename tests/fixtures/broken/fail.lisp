(test "wrong answer"
  (given filing-status 'single)
  (empty w2s)
  (expect taxable-income $1))

(test "bad forms"
  (given nope 1)
  (given filing-status $5)
  (expect wages $0)
  (frobnicate))
