use std::fs;
use std::path::PathBuf;

use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;

/// Keypair file format: JSON array of 64 bytes (same as Solana CLI's id.json).
/// First 32 bytes = secret key, last 32 bytes = public key.
const KEYPAIR_FILE: &str = "keypair.json";
const APP_DIR: &str = "anima";

/// Returns the path to ~/.anima/keypair.json (or keypair-{suffix}.json if specified).
fn keypair_path(suffix: Option<&str>) -> PathBuf {
    let home = dirs::home_dir().expect("Could not find home directory");
    let filename = match suffix {
        Some(s) => format!("keypair-{}.json", s),
        None => KEYPAIR_FILE.to_string(),
    };
    home.join(format!(".{}", APP_DIR)).join(filename)
}

/// Parse --keypair <suffix> from CLI args.
pub fn keypair_suffix_from_args() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|w| w[0] == "--keypair")
        .map(|w| w[1].clone())
}

/// Load an existing keypair from disk, or generate and save a new one.
/// Returns (signing_key, public_key_bytes).
pub fn load_or_create_keypair(suffix: Option<&str>) -> (SigningKey, [u8; 32]) {
    let path = keypair_path(suffix);

    if path.exists() {
        let data = fs::read_to_string(&path).expect("Failed to read keypair file");
        let bytes: Vec<u8> = serde_json::from_str(&data).expect("Failed to parse keypair JSON");
        assert_eq!(bytes.len(), 64, "Keypair file must be 64 bytes");

        let secret_bytes: [u8; 32] = bytes[..32].try_into().unwrap();
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        let pubkey = signing_key.verifying_key().to_bytes();

        bevy::log::info!(
            "Loaded keypair from {} — pubkey: {}",
            path.display(),
            bs58_encode(&pubkey)
        );

        (signing_key, pubkey)
    } else {
        let signing_key = SigningKey::generate(&mut OsRng);
        let pubkey = signing_key.verifying_key().to_bytes();

        // Save in Solana-compatible format: [secret_key(32) ++ public_key(32)]
        let mut keypair_bytes = Vec::with_capacity(64);
        keypair_bytes.extend_from_slice(&signing_key.to_bytes());
        keypair_bytes.extend_from_slice(&pubkey);

        let json = serde_json::to_string(&keypair_bytes).unwrap();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("Failed to create .anima directory");
        }
        fs::write(&path, &json).expect("Failed to write keypair file");

        bevy::log::info!(
            "Generated new keypair at {} — pubkey: {}",
            path.display(),
            bs58_encode(&pubkey)
        );

        (signing_key, pubkey)
    }
}

/// Derive a deterministic u64 client ID from the public key.
/// Used as the lightyear client ID for networking.
pub fn pubkey_to_client_id(pubkey: &[u8; 32]) -> u64 {
    u64::from_le_bytes(pubkey[..8].try_into().unwrap())
}

/// Get the public key as a Solana-style base58 address string.
pub fn pubkey_address(pubkey: &[u8; 32]) -> String {
    bs58_encode(pubkey)
}

/// Minimal base58 encoding (Solana address format).
fn bs58_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

    if bytes.is_empty() {
        return String::new();
    }

    // Count leading zeros
    let leading_zeros = bytes.iter().take_while(|&&b| b == 0).count();

    // Convert to base58
    let mut digits: Vec<u8> = Vec::new();
    for &byte in bytes {
        let mut carry = byte as u32;
        for digit in digits.iter_mut() {
            carry += (*digit as u32) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }

    let mut result = String::with_capacity(leading_zeros + digits.len());
    for _ in 0..leading_zeros {
        result.push('1');
    }
    for &d in digits.iter().rev() {
        result.push(ALPHABET[d as usize] as char);
    }
    result
}

/// Bevy resource holding the client's identity.
#[derive(bevy::prelude::Resource)]
pub struct ClientIdentity {
    pub signing_key: SigningKey,
    pub pubkey: [u8; 32],
    pub client_id: u64,
    pub address: String,
}

impl ClientIdentity {
    pub fn load_or_create() -> Self {
        let suffix = keypair_suffix_from_args();
        let (signing_key, pubkey) = load_or_create_keypair(suffix.as_deref());
        let client_id = pubkey_to_client_id(&pubkey);
        let address = pubkey_address(&pubkey);
        Self {
            signing_key,
            pubkey,
            client_id,
            address,
        }
    }
}
