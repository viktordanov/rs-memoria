/// The VAT rate in basis points: 2000 is 20%.
pub const VAT_BASIS_POINTS: u64 = 2000;

/// The VAT on a subtotal, rounded down to a whole cent.
pub fn vat(subtotal_cents: u64) -> u64 {
    subtotal_cents * VAT_BASIS_POINTS / 10_000
}
