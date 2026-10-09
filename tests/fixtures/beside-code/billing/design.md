# Billing design

This note explains the decisions behind billing. The [billing guide](README.md) lists the files.

<!-- memoria:section id="invoice" files="invoice.rs" -->
## Invoice lines

An invoice is a list of lines. Each line holds a description and an amount in cents.
Amounts are `u64` cents, so an amount is never negative and never a fraction of a cent.
The subtotal is the sum of all lines, before tax.
<!-- /memoria:section -->

<!-- memoria:section id="tax" files="tax.rs" -->
## Tax

Billing charges VAT at 20% of the subtotal.
The tax rounds down to a whole cent, so a customer never pays more than the rate.
<!-- /memoria:section -->
