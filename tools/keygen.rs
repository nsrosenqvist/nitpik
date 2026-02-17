//! One-time Ed25519 keypair generator for nitpick license signing.
//!
//! Run with: cargo run --example keygen
//!
//! Outputs:
//! - The private key (hex) — store this in your secrets vault, NEVER in the repo
//! - The public key as a Rust const — paste into src/license/mod.rs

fn main() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    eprintln!("=== PRIVATE KEY (store securely, NEVER commit) ===");
    println!("PRIVATE_KEY_HEX={}", hex::encode(signing_key.to_bytes()));

    eprintln!("\n=== PUBLIC KEY (paste into src/license/mod.rs) ===");
    let bytes = verifying_key.to_bytes();
    let formatted: Vec<String> = bytes.iter().map(|b| format!("0x{b:02x}")).collect();
    println!(
        "const PUBLIC_KEY_BYTES: [u8; 32] = [{}];",
        formatted.join(", ")
    );
}
