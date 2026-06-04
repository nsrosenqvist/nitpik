pub fn fold(bytes: &[u8]) -> u32 {
    let mut sum: u8 = 0;
    for &b in bytes {
        sum = sum.wrapping_add(b);
    }
    sum as u32
}
