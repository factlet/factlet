; A small program for the CLI tests: wages from W-2s, less a deduction.
(include "law.lisp")
(unit usd :prefix "$" :places 2)
(enum filing-status single married-joint)

(input filing-status : filing-status :label "Filing status")
(input interest : usd :default $0)
(collection w2s :label "Forms W-2"
  (input wages : usd :line 1)
  (input withheld : usd :line 2)
  (def rate (/ withheld wages)))

(def deduction
  (table filing-status
    single        law/deduction
    married-joint (* 2 law/deduction)))
(def taxable-income :label "Taxable income" :line 15
  (max $0 (- (+ (sum w2s wages) interest) deduction)))
