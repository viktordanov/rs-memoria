/// One line on an invoice.
pub struct Line {
    pub description: String,
    pub cents: u64,
}

/// The sum of all lines, before tax.
pub fn subtotal(lines: &[Line]) -> u64 {
    lines.iter().map(|line| line.cents).sum()
}
