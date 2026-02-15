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

/// Divide two numbers.
pub fn divide(a: i64, b: i64) -> i64 {
    // BUG: no check for division by zero
    a / b
}

/// Load a number from a file.
pub fn load_number(path: &str) -> i64 {
    let content = fs::read_to_string(path).unwrap();
    content.trim().parse().unwrap()
}
