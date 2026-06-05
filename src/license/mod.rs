//! Online license verification using signed entitlement JWTs.
//!
//! # Bounded Context: Licensing
//!
//! The CLI exchanges a user-supplied `NITPIK_LICENSE_KEY` (a long-lived
//! API key, format `nkp_live_…`) for a short-lived Ed25519-signed
//! entitlement JWT issued by `nitpik.dev`. The JWT is cached locally
//! at `~/.config/nitpik/entitlement.json` and re-fetched after it
//! expires (7 days by default).
//!
//! Verification is fully offline: the binary embeds the trusted public
//! keys keyed by `kid`. A compromised nitpik.dev server cannot forge
//! entitlements — it can only stop issuing valid ones.
//!
//! `NITPIK_OFFLINE_TOKEN` short-circuits the entire fetch path for
//! air-gapped CI environments: the CLI just verifies signature + exp
//! locally.
//!
//! # JWT shape
//!
//! ```json
//! header:  { "alg": "EdDSA", "typ": "JWT", "kid": "ed25519-2026-01" }
//! payload: { "iss": "https://nitpik.dev", "sub": "<user_id>",
//!            "iat": 1700000000, "exp": 1700604800,
//!            "subscription_id": "sub_…", "plan": "monthly",
//!            "type": "online" }     // or "offline"
//! ```

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

// ── Bundled trusted public keys ────────────────────────────────────────

/// A signing key the CLI trusts. New `kid`s are added here when the
/// server rotates; the old `kid` stays until any tokens it signed expire.
struct TrustedKey {
    kid: &'static str,
    bytes: [u8; 32],
}

/// Ed25519 public keys the CLI trusts. The matching seed is held only
/// in the nitpik.dev Worker as `ENTITLEMENT_SIGNING_SEED`. Add a new
/// entry before deploying a server-side rotation.
///
/// To replace this with a production key, run
/// `node scripts/derive-public-key.mjs --generate` in the nitpik-web
/// repo and paste the resulting 32-byte raw key bytes here.
/// Issuer claim the CLI requires on every entitlement JWT. A token
/// signed by a trusted key but issued by some other service is rejected.
const EXPECTED_ISS: &str = "https://nitpik.dev";

/// How long a previously-valid entitlement keeps working after its `exp`
/// when nitpik.dev can't be reached to refresh it. This decouples the
/// (short) refresh/metering cadence from outage resilience: a transient
/// nitpik.dev outage never fails a review — the cached entitlement is
/// honored for up to this window. Only a *confirmed* negative answer
/// (revoked key / inactive subscription) downgrades immediately; an
/// inability to reach us does not. 14 days comfortably spans any realistic
/// outage while still bounding how long a since-canceled subscription could
/// coast offline.
const OFFLINE_GRACE_SECONDS: i64 = 14 * 24 * 60 * 60;

const TRUSTED_KEYS: &[TrustedKey] = &[TrustedKey {
    kid: "ed25519-2026-01",
    bytes: [
        0xd0, 0x4a, 0xb2, 0x32, 0x74, 0x2b, 0xb4, 0xab, 0x3a, 0x13, 0x68, 0xbd, 0x46, 0x15, 0xe4,
        0xe6, 0xd0, 0x22, 0x4a, 0xb7, 0x1a, 0x01, 0x6b, 0xaf, 0x85, 0x20, 0xa3, 0x32, 0xc9, 0x77,
        0x87, 0x37,
    ],
}];

// ── Public types ───────────────────────────────────────────────────────

/// Parsed and verified entitlement claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseClaims {
    pub user_id: String,
    pub subscription_id: String,
    pub plan: String,
    /// Unix-seconds expiration claim from the JWT.
    pub expires_at: i64,
    pub kind: TokenKind,
    pub kid: String,
}

/// Whether an entitlement came from the live online flow or from a
/// downloadable offline token for air-gapped CI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenKind {
    Online,
    Offline,
}

#[derive(Error, Debug)]
pub enum LicenseError {
    #[error("invalid JWT format")]
    InvalidJwtFormat,

    #[error("invalid signature")]
    InvalidSignature,

    #[error("unknown signing key id: {0}")]
    UnknownKid(String),

    #[error("entitlement has expired")]
    Expired,

    #[error("invalid license key format")]
    InvalidKeyFormat,

    #[error("license key not recognized")]
    UnknownKey,

    #[error("subscription is not active")]
    SubscriptionInactive,

    #[error("network error: {0}")]
    Network(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid claims: {0}")]
    InvalidClaims(String),

    #[error("invalid base64: {0}")]
    InvalidBase64(String),
}

// ── License key format ─────────────────────────────────────────────────

/// Validate the user-facing format of a license key.
///
/// Format: `nkp_(live|test)_<26 Crockford-base32 chars>` (no `I`, `L`,
/// `O`, `U` to avoid visual ambiguity).
pub fn is_valid_key_format(key: &str) -> bool {
    let key = key.trim();
    let body = if let Some(b) = key.strip_prefix("nkp_live_") {
        b
    } else if let Some(b) = key.strip_prefix("nkp_test_") {
        b
    } else {
        return false;
    };
    body.len() == 26
        && body.chars().all(|c| {
            c.is_ascii_digit()
                || (c.is_ascii_uppercase() && c != 'I' && c != 'L' && c != 'O' && c != 'U')
        })
}

// ── JWT verification ───────────────────────────────────────────────────

fn b64url_decode(s: &str) -> Result<Vec<u8>, LicenseError> {
    use base64::{Engine, engine::general_purpose};
    general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| LicenseError::InvalidBase64(e.to_string()))
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct JwtPayload {
    iss: String,
    sub: String,
    #[allow(dead_code)]
    iat: i64,
    exp: i64,
    subscription_id: String,
    plan: String,
    #[serde(rename = "type")]
    token_type: String,
}

/// Verify a JWT's signature against a bundled public key (selected by
/// the `kid` header) and parse the claims. Also checks that `iss`
/// matches [`EXPECTED_ISS`] and that `exp` is in the future with
/// ±5 minutes of skew tolerance.
pub fn verify_jwt(jwt: &str) -> Result<LicenseClaims, LicenseError> {
    verify_jwt_inner(jwt, true)
}

/// Like [`verify_jwt`] but does **not** reject an expired token. The
/// signature, `kid`, issuer, and claim shape are still fully validated —
/// only the `exp` freshness check is skipped. Used by the offline-grace
/// path: a previously-valid entitlement stays trustworthy enough to keep
/// reviews running when nitpik.dev is temporarily unreachable. The caller
/// is responsible for bounding how stale a token it will accept.
pub fn verify_jwt_allow_expired(jwt: &str) -> Result<LicenseClaims, LicenseError> {
    verify_jwt_inner(jwt, false)
}

fn verify_jwt_inner(jwt: &str, check_exp: bool) -> Result<LicenseClaims, LicenseError> {
    let parts: Vec<&str> = jwt.trim().split('.').collect();
    if parts.len() != 3 {
        return Err(LicenseError::InvalidJwtFormat);
    }
    let header_bytes = b64url_decode(parts[0])?;
    let header: JwtHeader = serde_json::from_slice(&header_bytes)
        .map_err(|e| LicenseError::InvalidClaims(e.to_string()))?;
    if header.alg != "EdDSA" {
        return Err(LicenseError::InvalidJwtFormat);
    }

    let key_bytes = TRUSTED_KEYS
        .iter()
        .find(|k| k.kid == header.kid)
        .ok_or_else(|| LicenseError::UnknownKid(header.kid.clone()))?
        .bytes;
    let verify_key =
        VerifyingKey::from_bytes(&key_bytes).map_err(|_| LicenseError::InvalidSignature)?;

    let sig_bytes = b64url_decode(parts[2])?;
    let signature =
        Signature::from_slice(&sig_bytes).map_err(|_| LicenseError::InvalidSignature)?;

    let signing_input = format!("{}.{}", parts[0], parts[1]);
    verify_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| LicenseError::InvalidSignature)?;

    let payload_bytes = b64url_decode(parts[1])?;
    let payload: JwtPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|e| LicenseError::InvalidClaims(e.to_string()))?;

    if payload.iss != EXPECTED_ISS {
        return Err(LicenseError::InvalidClaims(format!(
            "unexpected issuer: {}",
            payload.iss
        )));
    }

    // ±5 min skew tolerance.
    if check_exp {
        let now = current_unix_seconds();
        if payload.exp + 300 < now {
            return Err(LicenseError::Expired);
        }
    }

    let kind = match payload.token_type.as_str() {
        "online" => TokenKind::Online,
        "offline" => TokenKind::Offline,
        other => {
            return Err(LicenseError::InvalidClaims(format!(
                "unknown token type: {other}"
            )));
        }
    };

    Ok(LicenseClaims {
        user_id: payload.sub,
        subscription_id: payload.subscription_id,
        plan: payload.plan,
        expires_at: payload.exp,
        kind,
        kid: header.kid,
    })
}

// ── Cache file ─────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct CachedEntitlement {
    token: String,
    fetched_at: String,
    exp: String,
    /// First 16 hex chars of sha256(license_key). Invalidates the cache
    /// when the user rotates their API key.
    api_key_fingerprint: String,
}

fn cache_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| {
        d.join(crate::constants::CONFIG_DIR)
            .join(crate::constants::ENTITLEMENT_CACHE_FILENAME)
    })
}

/// First 16 hex chars of sha256(key). Used to bind a cached entitlement
/// to a specific license key so rotation invalidates the cache.
pub fn key_fingerprint(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    hex::encode(&digest[..8])
}

/// Read a cached entitlement, but only return its token if the key
/// fingerprint matches (i.e. the cache was written for *this* key).
pub fn read_cache(api_key: &str) -> Option<String> {
    let path = cache_path()?;
    if !path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(&path).ok()?;
    let cached: CachedEntitlement = serde_json::from_str(&raw).ok()?;
    if cached.api_key_fingerprint != key_fingerprint(api_key) {
        return None;
    }
    Some(cached.token)
}

/// Persist a verified entitlement to disk. Best-effort permissions
/// hardening on Unix (`0600`).
pub fn write_cache(token: &str, api_key: &str, exp_iso: &str) -> std::io::Result<()> {
    let path = cache_path()
        .ok_or_else(|| std::io::Error::other("could not determine config directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let cached = CachedEntitlement {
        token: token.to_string(),
        fetched_at: current_iso_8601(),
        exp: exp_iso.to_string(),
        api_key_fingerprint: key_fingerprint(api_key),
    };
    let json = serde_json::to_string_pretty(&cached)
        .map_err(|e| std::io::Error::other(format!("serialize cache: {e}")))?;
    std::fs::write(&path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Best-effort cache deletion. Missing file is not an error.
pub fn clear_cache() -> std::io::Result<()> {
    if let Some(path) = cache_path()
        && path.exists()
    {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

// ── Server fetch ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct EntitlementResponse {
    token: String,
    exp: String,
}

/// Exchange a license key for a freshly signed entitlement JWT.
///
/// Returns `(token, exp_iso)`. The caller is expected to verify the
/// token's signature locally before caching or trusting it.
pub async fn fetch_entitlement(
    api_key: &str,
    api_url: &str,
) -> Result<(String, String), LicenseError> {
    let client =
        crate::http::build_client().map_err(|e| LicenseError::Network(format!("client: {e}")))?;
    let url = format!(
        "{}{}",
        api_url.trim_end_matches('/'),
        crate::constants::CLI_ENTITLEMENT_PATH
    );
    let res = client
        .post(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| LicenseError::Network(e.to_string()))?;

    match res.status().as_u16() {
        200 => {
            let body: EntitlementResponse = res
                .json()
                .await
                .map_err(|e| LicenseError::Network(format!("decode: {e}")))?;
            Ok((body.token, body.exp))
        }
        401 => Err(LicenseError::UnknownKey),
        403 => Err(LicenseError::SubscriptionInactive),
        s => Err(LicenseError::Network(format!("HTTP {s}"))),
    }
}

// ── Top-level entry point ──────────────────────────────────────────────

/// Resolve the current entitlement using the following precedence:
///
/// 1. `NITPIK_OFFLINE_TOKEN` env var (verified locally, no network).
/// 2. Cached JWT bound to the user's license key fingerprint.
/// 3. Fresh fetch via [`fetch_entitlement`], result cached on success.
///
/// Returns `None` (with a stderr warning) when no entitlement can be
/// established. Errors are intentionally non-fatal — nitpik downgrades
/// to the free-tier behavior rather than blocking the review.
pub async fn verify_entitlement(
    config: &crate::config::Config,
    env: &crate::env::Env,
) -> Option<LicenseClaims> {
    if let Ok(token) = env.var(crate::constants::ENV_OFFLINE_TOKEN) {
        let t = token.trim();
        if !t.is_empty() {
            return match verify_jwt(t) {
                Ok(claims) => Some(claims),
                Err(e) => {
                    eprintln!("Warning: NITPIK_OFFLINE_TOKEN is invalid: {e}");
                    None
                }
            };
        }
    }

    let api_key = config.license.key.as_ref()?;
    if !is_valid_key_format(api_key) {
        eprintln!("Warning: license key has invalid format (expected nkp_live_… or nkp_test_…)");
        return None;
    }

    if let Some(cached_token) = read_cache(api_key)
        && let Ok(claims) = verify_jwt(&cached_token)
    {
        return Some(claims);
    }
    // Cache verification failed (expired / kid rotated) — fall through to refetch.

    let api_url = env
        .var(crate::constants::ENV_API_URL)
        .unwrap_or_else(|_| crate::constants::DEFAULT_API_URL.to_string());

    match fetch_entitlement(api_key, &api_url).await {
        Ok((token, exp_iso)) => match verify_jwt(&token) {
            Ok(claims) => {
                let _ = write_cache(&token, api_key, &exp_iso);
                Some(claims)
            }
            Err(e) => {
                eprintln!("Warning: server returned an unverifiable entitlement: {e}");
                None
            }
        },
        Err(LicenseError::UnknownKey) => {
            eprintln!(
                "Warning: license key not recognized. Generate a new one at https://nitpik.dev/account."
            );
            None
        }
        Err(LicenseError::SubscriptionInactive) => {
            eprintln!("Warning: subscription is not active. Manage at https://nitpik.dev/account.");
            None
        }
        Err(LicenseError::Network(msg)) => {
            // Outage path: fail open. If we still hold a recently-valid
            // entitlement (signature/issuer intact, expired within the grace
            // window), keep the review licensed rather than punishing the
            // customer for our downtime. A confirmed negative (UnknownKey /
            // SubscriptionInactive above) does NOT reach here — those downgrade.
            if let Some(cached_token) = read_cache(api_key)
                && let Ok(claims) = verify_jwt_allow_expired(&cached_token)
                && current_unix_seconds() < claims.expires_at + OFFLINE_GRACE_SECONDS
            {
                eprintln!(
                    "Warning: could not reach nitpik.dev to refresh license ({msg}); \
                     using cached entitlement (grace period)."
                );
                return Some(claims);
            }
            eprintln!("Warning: could not reach nitpik.dev to verify license: {msg}");
            None
        }
        Err(e) => {
            eprintln!("Warning: license verification failed: {e}");
            None
        }
    }
}

// ── Date helpers (no chrono / time dep) ────────────────────────────────

fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn current_iso_8601() -> String {
    let epoch = current_unix_seconds();
    let (y, m, d, h, mi, s) = epoch_to_ymdhms(epoch);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Convert Unix-seconds to (Y, M, D, h, m, s) in UTC.
/// Howard Hinnant's civil_from_days algorithm.
fn epoch_to_ymdhms(epoch: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = epoch.div_euclid(86400);
    let rem = epoch.rem_euclid(86400);
    let h = rem / 3600;
    let mi = (rem % 3600) / 60;
    let s = rem % 60;
    let mut days = days + 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    // Suppress unused-mut lint if the assignment branch above is the only mutation.
    let _ = &mut days;
    (y, m, d, h, mi, s)
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose};
    use ed25519_dalek::SigningKey;

    fn signing_keypair() -> (SigningKey, [u8; 32]) {
        // Same 32-byte seed the test bundles via TRUSTED_KEYS.
        let seed = [
            0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
            0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
            0x11, 0x11, 0x11, 0x11,
        ];
        let sk = SigningKey::from_bytes(&seed);
        let pk = sk.verifying_key().to_bytes();
        (sk, pk)
    }

    /// Manually sign a JWT for tests — mirrors what the Worker does.
    fn sign_test_jwt(sk: &SigningKey, kid: &str, payload_json: &str) -> String {
        use ed25519_dalek::Signer;
        let header = format!(r#"{{"alg":"EdDSA","typ":"JWT","kid":"{kid}"}}"#);
        let h_b64 = general_purpose::URL_SAFE_NO_PAD.encode(header.as_bytes());
        let p_b64 = general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let signing_input = format!("{h_b64}.{p_b64}");
        let sig = sk.sign(signing_input.as_bytes());
        let s_b64 = general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{h_b64}.{p_b64}.{s_b64}")
    }

    fn future_exp() -> i64 {
        current_unix_seconds() + 7 * 24 * 60 * 60
    }

    #[test]
    fn bundled_public_key_matches_test_seed() {
        let (_, pk) = signing_keypair();
        assert_eq!(TRUSTED_KEYS[0].bytes, pk);
    }

    #[test]
    fn valid_key_format_accepts_well_formed_keys() {
        assert!(is_valid_key_format("nkp_live_K7Q3X8M2P9D4F6N1H5R0T2V8WJ"));
        assert!(is_valid_key_format("nkp_test_K7Q3X8M2P9D4F6N1H5R0T2V8WJ"));
    }

    #[test]
    fn valid_key_format_rejects_garbage() {
        assert!(!is_valid_key_format(""));
        assert!(!is_valid_key_format("nkp_live_TOOSHORT"));
        assert!(!is_valid_key_format("not-a-key"));
        // Lowercase body
        assert!(!is_valid_key_format("nkp_live_k7q3x8m2p9d4f6n1h5r0t2v8wj"));
        // Forbidden ambiguous chars (I, L, O, U)
        assert!(!is_valid_key_format("nkp_live_IAAAAAAAAAAAAAAAAAAAAAAAAA"));
    }

    #[test]
    fn verify_jwt_accepts_valid_online_token() {
        let (sk, _) = signing_keypair();
        let payload = format!(
            r#"{{"iss":"https://nitpik.dev","sub":"usr_1","iat":1700000000,"exp":{},"subscription_id":"sub_x","plan":"monthly","type":"online"}}"#,
            future_exp()
        );
        let jwt = sign_test_jwt(&sk, "ed25519-2026-01", &payload);
        let claims = verify_jwt(&jwt).unwrap();
        assert_eq!(claims.user_id, "usr_1");
        assert_eq!(claims.subscription_id, "sub_x");
        assert_eq!(claims.plan, "monthly");
        assert_eq!(claims.kind, TokenKind::Online);
        assert_eq!(claims.kid, "ed25519-2026-01");
    }

    #[test]
    fn verify_jwt_accepts_offline_type() {
        let (sk, _) = signing_keypair();
        let payload = format!(
            r#"{{"iss":"https://nitpik.dev","sub":"usr_1","iat":1700000000,"exp":{},"subscription_id":"sub_x","plan":"yearly","type":"offline"}}"#,
            future_exp()
        );
        let jwt = sign_test_jwt(&sk, "ed25519-2026-01", &payload);
        let claims = verify_jwt(&jwt).unwrap();
        assert_eq!(claims.kind, TokenKind::Offline);
        assert_eq!(claims.plan, "yearly");
    }

    #[test]
    fn verify_jwt_rejects_expired_token() {
        let (sk, _) = signing_keypair();
        let past_exp = current_unix_seconds() - 24 * 60 * 60;
        let payload = format!(
            r#"{{"iss":"https://nitpik.dev","sub":"usr_1","iat":0,"exp":{past_exp},"subscription_id":"sub_x","plan":"monthly","type":"online"}}"#
        );
        let jwt = sign_test_jwt(&sk, "ed25519-2026-01", &payload);
        assert!(matches!(verify_jwt(&jwt), Err(LicenseError::Expired)));
    }

    #[test]
    fn verify_jwt_allow_expired_accepts_recently_expired_token() {
        // The offline-grace primitive: an expired-but-otherwise-valid token
        // verifies (signature/issuer intact). verify_jwt rejects it; the
        // allow-expired variant returns the claims so the caller can apply
        // its own grace bound.
        let (sk, _) = signing_keypair();
        let past_exp = current_unix_seconds() - 24 * 60 * 60;
        let payload = format!(
            r#"{{"iss":"https://nitpik.dev","sub":"usr_grace","iat":0,"exp":{past_exp},"subscription_id":"sub_x","plan":"monthly","type":"online"}}"#
        );
        let jwt = sign_test_jwt(&sk, "ed25519-2026-01", &payload);
        assert!(matches!(verify_jwt(&jwt), Err(LicenseError::Expired)));
        let claims = verify_jwt_allow_expired(&jwt).expect("expired-but-signed token verifies");
        assert_eq!(claims.user_id, "usr_grace");
        assert_eq!(claims.expires_at, past_exp);
        // The grace bound the caller applies: still within the window.
        assert!(current_unix_seconds() < claims.expires_at + OFFLINE_GRACE_SECONDS);
    }

    #[test]
    fn verify_jwt_allow_expired_still_rejects_bad_signature() {
        // Grace must not weaken signature/issuer checks — only the exp check.
        let (sk, _) = signing_keypair();
        let payload =
            r#"{"iss":"https://evil.example","sub":"x","iat":0,"exp":0,"subscription_id":"s","plan":"monthly","type":"online"}"#;
        let jwt = sign_test_jwt(&sk, "ed25519-2026-01", payload);
        // Wrong issuer is still rejected even when ignoring exp.
        assert!(matches!(
            verify_jwt_allow_expired(&jwt),
            Err(LicenseError::InvalidClaims(_))
        ));

        // Tampered signature is rejected.
        let mut parts: Vec<&str> = jwt.split('.').collect();
        let tampered = format!("{}.{}.{}", parts[0], parts[1], "AAAA");
        parts.clear();
        assert!(verify_jwt_allow_expired(&tampered).is_err());
    }

    #[test]
    fn beyond_grace_window_is_not_honored() {
        // A token expired longer ago than the grace window must fall outside
        // the bound the outage path checks.
        let expired_long_ago = current_unix_seconds() - OFFLINE_GRACE_SECONDS - 24 * 60 * 60;
        assert!(current_unix_seconds() >= expired_long_ago + OFFLINE_GRACE_SECONDS);
    }

    #[test]
    fn verify_jwt_rejects_unknown_kid() {
        let (sk, _) = signing_keypair();
        let payload = format!(
            r#"{{"iss":"https://nitpik.dev","sub":"usr_1","iat":0,"exp":{},"subscription_id":"sub_x","plan":"monthly","type":"online"}}"#,
            future_exp()
        );
        let jwt = sign_test_jwt(&sk, "future-key-id", &payload);
        assert!(matches!(verify_jwt(&jwt), Err(LicenseError::UnknownKid(_))));
    }

    #[test]
    fn verify_jwt_rejects_wrong_issuer() {
        let (sk, _) = signing_keypair();
        let payload = format!(
            r#"{{"iss":"https://evil.example","sub":"usr_1","iat":0,"exp":{},"subscription_id":"sub_x","plan":"monthly","type":"online"}}"#,
            future_exp()
        );
        let jwt = sign_test_jwt(&sk, "ed25519-2026-01", &payload);
        match verify_jwt(&jwt) {
            Err(LicenseError::InvalidClaims(msg)) => {
                assert!(msg.contains("issuer"), "expected issuer error, got: {msg}");
            }
            other => panic!("expected InvalidClaims, got: {other:?}"),
        }
    }

    #[test]
    fn verify_jwt_rejects_tampered_payload() {
        let (sk, _) = signing_keypair();
        let payload = format!(
            r#"{{"iss":"https://nitpik.dev","sub":"usr_1","iat":0,"exp":{},"subscription_id":"sub_x","plan":"monthly","type":"online"}}"#,
            future_exp()
        );
        let jwt = sign_test_jwt(&sk, "ed25519-2026-01", &payload);

        // Swap in a different payload while keeping the original signature.
        let parts: Vec<&str> = jwt.split('.').collect();
        let tampered_payload = general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"iss":"https://nitpik.dev","sub":"attacker","iat":0,"exp":9999999999,"subscription_id":"sub_x","plan":"monthly","type":"online"}"#.as_bytes(),
        );
        let tampered = format!("{}.{}.{}", parts[0], tampered_payload, parts[2]);
        assert!(matches!(
            verify_jwt(&tampered),
            Err(LicenseError::InvalidSignature)
        ));
    }

    #[test]
    fn verify_jwt_rejects_garbage() {
        assert!(matches!(
            verify_jwt("not.a.jwt"),
            Err(LicenseError::InvalidBase64(_))
        ));
        assert!(matches!(
            verify_jwt("only-one-part"),
            Err(LicenseError::InvalidJwtFormat)
        ));
    }

    #[test]
    fn key_fingerprint_is_stable_and_short() {
        let f1 = key_fingerprint("nkp_live_K7Q3X8M2P9D4F6N1H5R0T2V8WJ");
        let f2 = key_fingerprint("nkp_live_K7Q3X8M2P9D4F6N1H5R0T2V8WJ");
        let f3 = key_fingerprint("nkp_live_DIFFERENT00000000000000000");
        assert_eq!(f1, f2);
        assert_ne!(f1, f3);
        assert_eq!(f1.len(), 16);
    }

    #[test]
    fn iso8601_format_round_trip() {
        let s = current_iso_8601();
        // YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
        assert!(s.chars().nth(4) == Some('-'));
        assert!(s.chars().nth(10) == Some('T'));
    }
}
