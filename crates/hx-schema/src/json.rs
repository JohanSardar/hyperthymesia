//! JSON representations shared by persistence and disclosure.

use serde_json::Number;

/// Legacy stored number text. Secret filtering must check the same expansion
/// that persistence writes, not only serde's exponent-form representation.
pub fn stored_number(number: &Number) -> String {
    let text = number.to_string();
    if let Some((mantissa, exponent)) = text.split_once(['e', 'E']) {
        let exponent: i32 = exponent.parse().expect("JSON number exponent");
        let negative = mantissa.starts_with('-');
        let mantissa = mantissa.trim_start_matches('-');
        let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
        let digits = mantissa.replace('.', "");
        let expanded = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point as usize >= digits.len() {
            format!("{}{}", digits, "0".repeat(point as usize - digits.len()))
        } else {
            format!(
                "{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        };
        if negative {
            format!("-{expanded}")
        } else {
            expanded
        }
    } else if text.starts_with('-') && number.as_f64() == Some(0.0) {
        text.trim_start_matches('-').to_owned()
    } else {
        text
    }
}
