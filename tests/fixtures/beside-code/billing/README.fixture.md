# Billing

Billing turns invoice lines into a total with tax.

- `invoice.rs` holds the invoice lines and the subtotal.
- `tax.rs` computes the VAT on the subtotal.
- The [design note](design.md) explains the decisions behind both files.
