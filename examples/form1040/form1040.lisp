; A slice of TY2025 Form 1040: wages from any number of W-2s, taxable
; interest, the standard deduction, regular tax, and withholding.
;
; Simplified for illustration: tax uses the exact bracket formula rounded to
; the dollar (not the Tax Table midpoints under $100,000), and there are no
; credits, other income, or age/blindness additions.

(enum filing-status
  single married-joint married-separate head-of-household
  qualifying-surviving-spouse)

; ---- Answers ----

(input filing-status : filing-status)
(collection w2s
  (input box1-wages : usd)
  (input box2-withheld : usd))
(input taxable-interest : usd)

; ---- Law ----

; IRC §63(c)(2) as amended by P.L. 119-21 §70102, for 2025.
(def law/std-deduction/single $15,750)
(def law/std-deduction/joint $31,500)
(def law/std-deduction/hoh $23,625)

; Rev. Proc. 2024-40 §3.01, unchanged for 2025 by P.L. 119-21: each rate
; applies up to the edge after it.
(def law/brackets/single
  [10% $11,925  12% $48,475  22% $103,350  24% $197,300  32% $250,525  35% $626,350  37%])
(def law/brackets/joint
  [10% $23,850  12% $96,950  22% $206,700  24% $394,600  32% $501,050  35% $751,600  37%])
(def law/brackets/separate
  [10% $11,925  12% $48,475  22% $103,350  24% $197,300  32% $250,525  35% $375,800  37%])
(def law/brackets/hoh
  [10% $17,000  12% $64,850  22% $103,350  24% $197,300  32% $250,500  35% $626,350  37%])

; ---- Lines ----

(def line1z/wages (sum w2s box1-wages))
(def line11/agi (+ line1z/wages taxable-interest))
(def line12/standard-deduction
  (table filing-status
    (single married-separate)                   law/std-deduction/single
    (married-joint qualifying-surviving-spouse) law/std-deduction/joint
    head-of-household                           law/std-deduction/hoh))
(def line15/taxable-income
  (max $0 (- line11/agi line12/standard-deduction)))
(def tax-schedule
  (table filing-status
    single                                      law/brackets/single
    (married-joint qualifying-surviving-spouse) law/brackets/joint
    married-separate                            law/brackets/separate
    head-of-household                           law/brackets/hoh))
(def line16/tax (round $1 (brackets line15/taxable-income tax-schedule)))
(def line25a/withholding (sum w2s box2-withheld))
(def line34/refund (max $0 (- line25a/withholding line16/tax)))
(def line37/amount-owed (max $0 (- line16/tax line25a/withholding)))
