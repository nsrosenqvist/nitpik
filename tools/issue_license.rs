//! Issue a signed license key. Internal tool — never ship to users.
//!
//! Usage:
//!   NITPIK_SIGNING_KEY=<hex> cargo run --example issue_license -- \
//!     --name "Acme Corp" --id "acme-001" --expires "2027-02-13"

fn main() {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    let args: Vec<String> = std::env::args().collect();

    let name = get_arg(&args, "--name").expect("--name required");
    let id = get_arg(&args, "--id").expect("--id required");
    let expires = get_arg(&args, "--expires").expect("--expires required (YYYY-MM-DD)");

    let today = {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let days = secs / 86400;
        let (y, m, d) = epoch_days_to_ymd(days as i64);
        format!("{y:04}-{m:02}-{d:02}")
    };

    let private_hex =
        std::env::var("NITPIK_SIGNING_KEY").expect("NITPIK_SIGNING_KEY env var required");
    let private_bytes = hex::decode(&private_hex).expect("invalid hex in NITPIK_SIGNING_KEY");
    let key_bytes: [u8; 32] = private_bytes
        .try_into()
        .expect("signing key must be 32 bytes");
    let signing_key = SigningKey::from_bytes(&key_bytes);

    let payload = serde_json::json!({
        "customer_name": name,
        "customer_id": id,
        "issued_at": today,
        "expires_at": expires,
    });
    let payload_bytes = serde_json::to_vec(&payload).unwrap();

    let signature = signing_key.sign(&payload_bytes);

    let mut blob = payload_bytes;
    blob.extend_from_slice(&signature.to_bytes());

    let license_key = base64::engine::general_purpose::STANDARD.encode(&blob);

    eprintln!("Customer:   {name}");
    eprintln!("ID:         {id}");
    eprintln!("Issued:     {today}");
    eprintln!("Expires:    {expires}");
    eprintln!("---");
    println!("{license_key}");
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn epoch_days_to_ymd(days: i64) -> (i64, i64, i64) {
    let days = days + 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
