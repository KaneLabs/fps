use std::fs;
use std::path::PathBuf;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;

/// Keypair file format: JSON array of 64 bytes (same as Solana CLI's id.json).
/// First 32 bytes = secret key, last 32 bytes = public key.
const KEYPAIR_FILE: &str = "keypair.json";
const APP_DIR: &str = "anima";

/// Auth message prefix. The signed payload is: "ANIMA_AUTH_v1:{client_id}"
/// This proves the client controls the Ed25519 private key that generated
/// the public key from which client_id was derived.
const AUTH_MESSAGE_PREFIX: &str = "ANIMA_AUTH_v1";

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
            pubkey_address(&pubkey)
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
            pubkey_address(&pubkey)
        );

        (signing_key, pubkey)
    }
}

/// Derive a deterministic u64 from the public key.
///
/// NOTE: this is NO LONGER the netcode client ID — see `new_session_id()`. It
/// survives as the stable short display name for a wallet (the base58 of these
/// 8 bytes is the name players see in the kill feed).
pub fn pubkey_to_client_id(pubkey: &[u8; 32]) -> u64 {
    u64::from_le_bytes(pubkey[..8].try_into().unwrap())
}

/// Mint a fresh random netcode client ID for a new connection attempt.
///
/// The netcode client ID is a per-SESSION handle, deliberately NOT derived from
/// the wallet pubkey. Wallet-derived IDs made every reconnect from the same
/// wallet collide with its own still-lingering session: lightyear rejects the
/// new connection with `ClientIdInUse` for up to `client_timeout_secs`, and a
/// client retrying inside that window could land half-wired — inputs flowing
/// client→server while server→client replication was never applied (the 21:05
/// playtest P0, ConfirmedTick frozen for the whole session).
///
/// Random per-session IDs mean two sessions of the same wallet never collide,
/// so the rejection window — and the race inside it — cannot occur at all.
///
/// Identity is unaffected: the Ed25519 wallet keypair is still the durable
/// identity, proven per session by signing this ID (see `sign_auth_message`).
/// Kicking a stale session is therefore an AUTHENTICATED action at the app
/// layer, not an unauthenticated one at the netcode layer — see the kick-old
/// policy in `server.rs`. That distinction matters: connect tokens are minted
/// with a hardcoded all-zero private key, so anyone can forge a token for any
/// client ID. Kicking on raw ID collision would let anyone boot any player
/// knowing only their (public) wallet address.
pub fn new_session_id() -> u64 {
    use rand::RngCore;
    OsRng.next_u64()
}

/// Get the public key as a Solana-style base58 address string.
pub fn pubkey_address(pubkey: &[u8; 32]) -> String {
    bs58::encode(pubkey).into_string()
}

/// Base58-encode a u64 client ID (first 8 bytes of pubkey) for display.
pub fn client_id_to_base58(id: u64) -> String {
    bs58::encode(&id.to_le_bytes()).into_string()
}

// ========================================
// Wallet Auth — Challenge-Response
// ========================================

/// Construct the deterministic auth message for a given client_id.
/// Format: "ANIMA_AUTH_v1:{client_id}"
///
/// This is signed by the client and verified by the server to prove
/// ownership of the Ed25519 private key behind the Solana wallet address.
pub fn auth_message_for_client(client_id: u64) -> Vec<u8> {
    format!("{}:{}", AUTH_MESSAGE_PREFIX, client_id).into_bytes()
}

/// Sign the auth challenge message with the client's Ed25519 key.
/// Returns the 64-byte Ed25519 signature.
///
/// The signed message is deterministic: "ANIMA_AUTH_v1:{client_id}"
/// This proves the client controls the private key for the pubkey
/// from which client_id was derived.
pub fn sign_auth_message(signing_key: &SigningKey, client_id: u64) -> [u8; 64] {
    let message = auth_message_for_client(client_id);
    let signature = signing_key.sign(&message);
    signature.to_bytes()
}

/// Verify an auth signature from a client.
///
/// Checks that the Ed25519 signature is valid over "ANIMA_AUTH_v1:{client_id}",
/// where `claimed_client_id` MUST be supplied by the caller from the live
/// connection (`RemoteId`) — never from the message body.
///
/// Session IDs are random per connection (see `new_session_id`), so there is no
/// longer a pubkey→client_id derivation to check. The binding is now the
/// signature itself: only the holder of the wallet's private key can sign this
/// session's ID, and the server compares against the ID the connection actually
/// has. A client cannot claim a wallet it does not own.
///
/// KNOWN GAP (pre-existing, tracked separately): the signed message is
/// client-chosen, not a server-issued nonce, so a captured (pubkey, signature)
/// pair can be replayed by reconnecting with the same session ID. Closing this
/// needs a server→client challenge before auth.
///
/// Returns the full Solana address (base58 pubkey) on success.
pub fn verify_auth_signature(
    pubkey_bytes: &[u8; 32],
    signature_bytes: &[u8],
    claimed_client_id: u64,
) -> Result<String, AuthError> {
    // Signature must be exactly 64 bytes
    if signature_bytes.len() != 64 {
        return Err(AuthError::InvalidSignature);
    }

    // 2. Reconstruct verifying key from raw bytes
    let verifying_key = VerifyingKey::from_bytes(pubkey_bytes)
        .map_err(|_| AuthError::InvalidPubkey)?;

    // 3. Reconstruct signature from bytes
    let sig_array: [u8; 64] = signature_bytes.try_into().unwrap();
    let signature = Signature::from_bytes(&sig_array);

    // 4. Verify the signature over the deterministic auth message
    let message = auth_message_for_client(claimed_client_id);
    verifying_key
        .verify(&message, &signature)
        .map_err(|_| AuthError::InvalidSignature)?;

    // Success — return the Solana wallet address
    Ok(pubkey_address(pubkey_bytes))
}

/// Auth verification errors.
#[derive(Debug)]
pub enum AuthError {
    /// The public key bytes are not a valid Ed25519 point
    InvalidPubkey,
    /// The Ed25519 signature does not verify
    InvalidSignature,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::InvalidPubkey => write!(f, "invalid Ed25519 public key"),
            AuthError::InvalidSignature => write!(f, "Ed25519 signature verification failed"),
        }
    }
}

/// Bevy resource holding the client's identity.
///
/// `signing_key`/`pubkey`/`address` are the DURABLE identity (the wallet, loaded
/// from disk). `client_id` is an EPHEMERAL per-connection handle, re-minted for
/// every connection attempt — see `new_session()`.
#[derive(bevy::prelude::Resource)]
pub struct ClientIdentity {
    pub signing_key: SigningKey,
    pub pubkey: [u8; 32],
    /// Per-session netcode ID. Random, NOT wallet-derived. Re-minted on every
    /// connect attempt including self-heal reconnects.
    pub client_id: u64,
    pub address: String,
    /// Stable short name for this wallet, independent of the session ID.
    pub display_name: String,
}

impl ClientIdentity {
    pub fn load_or_create() -> Self {
        let suffix = keypair_suffix_from_args();
        let (signing_key, pubkey) = load_or_create_keypair(suffix.as_deref());
        let client_id = new_session_id();
        let address = pubkey_address(&pubkey);
        let display_name = client_id_to_base58(pubkey_to_client_id(&pubkey));
        Self {
            signing_key,
            pubkey,
            client_id,
            address,
            display_name,
        }
    }

    /// Mint a fresh session ID for a new connection attempt.
    ///
    /// MUST be called before every connect, including the [SELF-HEAL] reconnect.
    /// Reusing the ID there was self-defeating: the heal would collide with the
    /// very stale session it was trying to escape, hit `ClientIdInUse`, and risk
    /// landing back in the same half-wired state it was healing from.
    pub fn new_session(&mut self) -> u64 {
        self.client_id = new_session_id();
        self.client_id
    }

    /// Sign the auth challenge for this identity.
    /// Returns (pubkey_bytes, signature_bytes) to send to the server.
    pub fn sign_auth(&self) -> ([u8; 32], Vec<u8>) {
        let sig = sign_auth_message(&self.signing_key, self.client_id);
        (self.pubkey, sig.to_vec())
    }
}

/// Server-side resource tracking verified wallet addresses.
/// Maps client_id → verified Solana wallet address (base58).
#[derive(bevy::prelude::Resource, Default)]
pub struct VerifiedWallets {
    pub wallets: std::collections::HashMap<u64, String>,
}

impl VerifiedWallets {
    /// Check if a client has been wallet-verified.
    pub fn is_verified(&self, client_id: u64) -> bool {
        self.wallets.contains_key(&client_id)
    }

    /// Get the Solana address for a verified client.
    pub fn get_address(&self, client_id: u64) -> Option<&str> {
        self.wallets.get(&client_id).map(|s| s.as_str())
    }

    /// Remove a client's wallet verification (on disconnect).
    /// Returns true if the client was previously verified.
    pub fn remove(&mut self, client_id: u64) -> bool {
        self.wallets.remove(&client_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wallet authenticates over a RANDOM session ID (no pubkey derivation).
    #[test]
    fn test_sign_and_verify() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let pubkey = signing_key.verifying_key().to_bytes();
        let client_id = new_session_id();

        let sig = sign_auth_message(&signing_key, client_id);
        let result = verify_auth_signature(&pubkey, &sig, client_id);
        assert!(result.is_ok());
    }

    /// The signature is bound to the session ID the SERVER sees. A client that
    /// signs one ID cannot authenticate a connection carrying a different one.
    #[test]
    fn test_wrong_client_id_fails() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let pubkey = signing_key.verifying_key().to_bytes();
        let client_id = new_session_id();

        let sig = sign_auth_message(&signing_key, client_id);
        // Try to verify with wrong client_id
        let result = verify_auth_signature(&pubkey, &sig, client_id.wrapping_add(1));
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_signature_fails() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let pubkey = signing_key.verifying_key().to_bytes();
        let client_id = new_session_id();

        let mut sig = sign_auth_message(&signing_key, client_id);
        sig[0] ^= 0xFF; // corrupt signature
        let result = verify_auth_signature(&pubkey, &sig, client_id);
        assert!(result.is_err());
    }

    /// You cannot claim someone else's wallet: signing a session ID with your
    /// own key never verifies against a victim's pubkey. This is what makes the
    /// auth-gated kick-old policy safe — kicking requires proving the wallet.
    #[test]
    fn test_cannot_claim_another_wallet() {
        let victim = SigningKey::generate(&mut OsRng);
        let victim_pubkey = victim.verifying_key().to_bytes();
        let attacker = SigningKey::generate(&mut OsRng);

        let client_id = new_session_id();
        // Attacker signs the session ID with their OWN key, claims victim pubkey.
        let sig = sign_auth_message(&attacker, client_id);
        let result = verify_auth_signature(&victim_pubkey, &sig, client_id);
        assert!(result.is_err());
    }

    /// Session IDs must actually differ between connection attempts — this is
    /// the whole point of the fix (no self-collision on reconnect).
    #[test]
    fn test_session_ids_are_distinct() {
        let ids: std::collections::HashSet<u64> =
            (0..64).map(|_| new_session_id()).collect();
        assert_eq!(ids.len(), 64, "session IDs must be unique per mint");
    }

    /// Re-minting changes the session ID but never the wallet identity.
    #[test]
    fn test_new_session_preserves_wallet_identity() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let pubkey = signing_key.verifying_key().to_bytes();
        let mut identity = ClientIdentity {
            signing_key,
            pubkey,
            client_id: new_session_id(),
            address: pubkey_address(&pubkey),
            display_name: client_id_to_base58(pubkey_to_client_id(&pubkey)),
        };

        let before = identity.client_id;
        let address_before = identity.address.clone();
        let name_before = identity.display_name.clone();

        let after = identity.new_session();

        assert_ne!(before, after, "reconnect must mint a fresh session ID");
        assert_eq!(identity.pubkey, pubkey, "wallet key must survive reconnect");
        assert_eq!(identity.address, address_before);
        assert_eq!(identity.display_name, name_before);

        // ...and the new session still authenticates.
        let sig = sign_auth_message(&identity.signing_key, identity.client_id);
        assert!(verify_auth_signature(&identity.pubkey, &sig, identity.client_id).is_ok());
    }
}
