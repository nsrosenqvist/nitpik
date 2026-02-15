/// A simple calculator module.
use std::fs;

/// Add two numbers.
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Subtract two numbers.
pub fn subtract(a: i64, b: i64) -> i64 {
    a - b
}

/// Divide two numbers safely.
pub fn divide(a: i64, b: i64) -> Option<i64> {
    // Fixed: now returns None on division by zero
    if b == 0 {
        return None;
    }
    Some(a / b)
}

/// Load a number from a file.
pub fn load_number(path: &str) -> i64 {
    // Still uses unwrap — not properly fixed
    let content = fs::read_to_string(path).unwrap();
    content.trim().parse().unwrap()
}

/// Multiply, but with an overflow risk on large inputs.
pub fn multiply(a: i64, b: i64) -> i64 {
    a * b
}
