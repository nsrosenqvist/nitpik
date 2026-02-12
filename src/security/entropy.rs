//! Shannon entropy calculation.

/// Calculate the Shannon entropy of a string.
///
/// Higher entropy indicates more randomness, which is characteristic
/// of secrets and API keys. Typical thresholds:
/// - English text: ~3.5-4.5 bits
/// - Hex strings: ~3.0-4.0 bits
/// - Base64 strings: ~5.0-6.0 bits
/// - Random bytes (base64): ~5.5+ bits
pub fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }

    let len = s.len() as f64;
    let mut freq = [0u32; 256];

    for &byte in s.as_bytes() {
        freq[byte as usize] += 1;
    }

    freq.iter()
        .filter(|&&count| count > 0)
        .map(|&count| {
            let p = count as f64 / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string() {
        assert_eq!(shannon_entropy(""), 0.0);
    }

    #[test]
    fn single_char_repeated() {
        assert_eq!(shannon_entropy("aaaaaaa"), 0.0);
    }

    #[test]
    fn two_chars_equal() {
        let e = shannon_entropy("ab");
        assert!((e - 1.0).abs() < 0.01);
    }

    #[test]
    fn high_entropy_string() {
        // A random-looking API key should have high entropy
        let e = shannon_entropy("aB3xK9mQ2pL7wR5tY8nU4vC6jH0fE1s");
        assert!(e > 4.0, "entropy was {e}");
    }

    #[test]
    fn low_entropy_string() {
        // Repetitive text should have low entropy
        let e = shannon_entropy("password");
        assert!(e < 3.5, "entropy was {e}");
    }
}
