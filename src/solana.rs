//! Solana Integration Layer for Anima
//!
//! This module contains the token structure design and respawn authorization
//! system. The game's economy is built on Solana SPL tokens — every meaningful
//! in-game item is a token, and every economic action is a transaction.
//!
//! # Token Structure Design (SPL Token Architecture)
//!
//! ## Core Principle
//! Each item *type* in the game maps to an SPL Token **Mint**.
//! Each player's holdings of that item type are tracked via a **Token Account**
//! (ATA — Associated Token Account) owned by the player's wallet.
//!
//! ## Token Mints (one per item type)
//!
//! | Item Type       | Mint Name        | Decimals | Notes                              |
//! |-----------------|------------------|----------|------------------------------------|
//! | Raw Ore         | `ANIMA_ORE`      | 0        | Mined from ore blocks, fungible    |
//! | Refined Metal   | `ANIMA_METAL`    | 0        | Smelted from ore, crafting input   |
//! | AK-47           | `ANIMA_AK47`     | 0        | Non-fungible-ish (quantity = 1)    |
//! | Pickaxe         | `ANIMA_PICKAXE`  | 0        | Mining tool, fungible              |
//! | Respawn Token   | `ANIMA_RESPAWN`  | 0        | Burned on respawn (1 per death)    |
//! | SOL (native)    | —                | 9        | Gas + respawn fee fallback         |
//!
//! ### Design Decisions
//!
//! - **Decimals = 0** for all game items: items are discrete, not fractional.
//!   You can't have 0.5 of a pickaxe.
//!
//! - **Fungible tokens for resources** (ore, metal, respawn tokens): These are
//!   interchangeable. 10 ore is 10 ore regardless of who mined it.
//!
//! - **Fungible tokens with quantity semantics for weapons**: An AK-47 token
//!   with amount=1 in your ATA means you have one AK-47. We don't need NFTs
//!   for weapons unless we add unique properties (skins, wear, etc). When we
//!   do, we'll migrate to Metaplex Core or Token-2022 with metadata extension.
//!
//! ## Player Inventory = Token Accounts
//!
//! A player's inventory is the set of ATAs derived from their wallet pubkey:
//!
//! ```text
//! Player Wallet: 7xKXt...
//!   ├── ATA(ANIMA_ORE):      balance = 15   → 15 ore in inventory
//!   ├── ATA(ANIMA_AK47):     balance = 1    → has an AK-47
//!   ├── ATA(ANIMA_PICKAXE):  balance = 1    → has a pickaxe
//!   └── ATA(ANIMA_RESPAWN):  balance = 3    → 3 respawn tokens remaining
//! ```
//!
//! The game server reads these balances to determine what items the player can
//! use. The on-chain state IS the inventory — no separate database.
//!
//! ## Economic Actions as Transactions
//!
//! ### Mining (ore block → player inventory)
//! ```text
//! Instruction: MintTo(ANIMA_ORE, player_ata, amount=1)
//! Authority:   Game server (mint authority for all game mints)
//! Trigger:     Player completes 3-second mine on ore block
//! ```
//! The server holds the mint authority keypair. When a player finishes mining,
//! the server signs a MintTo instruction. The player doesn't need to sign —
//! they're receiving tokens, not spending them.
//!
//! ### Equip/Unequip (no transaction needed)
//! Equipping an item is a game-state change, not an economic action.
//! The player's ATA balance doesn't change when they hold the AK-47 vs
//! having it in inventory. The `PlayerEquipped` component tracks this.
//!
//! ### Death + Loot Drop
//! ```text
//! Instruction: Transfer(all player ATAs → loot_pool_ata, all balances)
//! Authority:   Player wallet (pre-authorized via delegate or session key)
//! Trigger:     Player health reaches 0
//! ```
//! On death, all items transfer to a loot pool. Other players who find the
//! loot drop can claim items. This is the "full loot" economy.
//!
//! **Session key approach**: On connect, the player signs a transaction that
//! delegates transfer authority to the server for their ATAs (with a cap).
//! The server can then execute transfers without per-transaction player signing.
//!
//! ### Respawn (pay to continue)
//! ```text
//! Option A - Burn respawn token:
//!   Instruction: Burn(ANIMA_RESPAWN, player_ata, amount=1)
//!   Authority:   Server (delegated)
//!
//! Option B - Pay SOL:
//!   Instruction: Transfer(player_wallet → treasury, 0.01 SOL)
//!   Authority:   Server (delegated)
//! ```
//! The `authorize_respawn()` function checks:
//! 1. Does the player have a RESPAWN token? → burn it, authorize
//! 2. Does the player have enough SOL? → transfer fee, authorize
//! 3. Neither? → deny respawn (permanent death until funded)
//!
//! ### Crafting (future)
//! ```text
//! Instruction: Burn(ANIMA_ORE, player_ata, 5) + MintTo(ANIMA_METAL, player_ata, 1)
//! Authority:   Server (crafting recipe validation)
//! ```
//!
//! ## Mint Authority Model
//!
//! The game server keypair is the **mint authority** for all game token mints.
//! This means:
//! - Only the server can mint new tokens (mining rewards, quest rewards)
//! - Players can transfer tokens between themselves (trading, looting)
//! - The server can burn tokens with delegated authority (respawn cost)
//!
//! In production, the mint authority should be a multisig (server + admin)
//! or a program-derived address (PDA) from an Anchor program that enforces
//! game rules on-chain.
//!
//! ## Future: On-Chain Program (Anchor)
//!
//! The long-term architecture moves game logic into a Solana program:
//! ```text
//! Program: anima_game
//!   - mine(player, ore_block_id) → MintTo(ORE, player_ata, 1)
//!   - respawn(player) → Burn(RESPAWN, player_ata, 1)
//!   - craft(player, recipe_id) → Burn inputs + Mint outputs
//!   - loot(killer, victim) → Transfer all victim ATAs to killer
//! ```
//! This moves trust from the server to the blockchain. The server becomes
//! a relayer that submits transactions, not an authority that controls mints.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

// ========================================
// Respawn Authorization
// ========================================

/// How respawns are gated against the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaymentMode {
    /// Free respawns; the chain is never consulted. Local dev default.
    #[default]
    Off,
    /// Respawn requires the verified wallet to HOLD >= `respawn_cost_lamports`
    /// on-chain. No transfer happens — respawn friction without payment
    /// friction. Staging / playtest mode.
    BalanceGate,
    /// True pay-per-respawn: the client signs and submits a SOL transfer to
    /// the treasury; the server verifies the transaction on-chain before
    /// authorizing (server-as-verifier). Production mode.
    Paid,
}

impl PaymentMode {
    /// Parse "off" | "balance" | "paid" (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "balance" => Some(Self::BalanceGate),
            "paid" => Some(Self::Paid),
            _ => None,
        }
    }
}

/// Configuration for the pay-to-respawn system.
///
/// See `PaymentMode` for the three gating modes. The cost applies to both
/// gated modes: the balance a wallet must hold (BalanceGate) or the amount
/// it must transfer to the treasury (Paid).
#[derive(Resource, Clone, Debug)]
pub struct RespawnConfig {
    /// How respawns are gated. Off in local dev.
    pub payment_mode: PaymentMode,

    /// Respawn cost in lamports. Default: 10_000_000 (0.01 SOL).
    pub respawn_cost_lamports: u64,

    /// Solana RPC endpoint for on-chain checks.
    /// Default: "http://localhost:8899" (local validator)
    pub rpc_url: String,

    /// Treasury wallet address that receives respawn SOL fees.
    pub treasury_address: String,
}

impl Default for RespawnConfig {
    fn default() -> Self {
        Self {
            // Dev mode: respawns are free
            payment_mode: PaymentMode::Off,
            // 0.01 SOL
            respawn_cost_lamports: 10_000_000,
            // Local validator
            rpc_url: "http://localhost:8899".to_string(),
            // Placeholder — set this to the actual treasury wallet in production
            treasury_address: "11111111111111111111111111111111".to_string(),
        }
    }
}

/// Parse respawn-payment config from env vars and CLI args.
/// Precedence: CLI flag > env var > default.
/// Env vars: ANIMA_PAYMENT_MODE (off|balance|paid), ANIMA_RPC_URL, ANIMA_TREASURY.
pub fn parse_respawn_config() -> RespawnConfig {
    let args: Vec<String> = std::env::args().collect();
    let mut config = RespawnConfig::default();

    // Env fallbacks (overridden by CLI flags below)
    if let Ok(mode) = std::env::var("ANIMA_PAYMENT_MODE") {
        match PaymentMode::parse(&mode) {
            Some(m) => config.payment_mode = m,
            None => bevy::log::warn!("ANIMA_PAYMENT_MODE '{mode}' invalid (off|balance|paid) — ignored"),
        }
    }
    if let Ok(url) = std::env::var("ANIMA_RPC_URL") {
        config.rpc_url = url;
    }
    if let Ok(addr) = std::env::var("ANIMA_TREASURY") {
        config.treasury_address = addr;
    }

    // Legacy alias from phase A: --require-respawn-payment == balance gate.
    if args.contains(&"--require-respawn-payment".to_string()) {
        config.payment_mode = PaymentMode::BalanceGate;
    }

    // Parse --respawn-payment <off|balance|paid>
    if let Some(pos) = args.iter().position(|a| a == "--respawn-payment") {
        if let Some(mode) = args.get(pos + 1) {
            match PaymentMode::parse(mode) {
                Some(m) => config.payment_mode = m,
                None => bevy::log::warn!("--respawn-payment '{mode}' invalid (off|balance|paid) — ignored"),
            }
        }
    }

    // Parse --rpc-url <url>
    if let Some(pos) = args.iter().position(|a| a == "--rpc-url") {
        if let Some(url) = args.get(pos + 1) {
            config.rpc_url = url.clone();
        }
    }

    // Parse --treasury <address>
    if let Some(pos) = args.iter().position(|a| a == "--treasury") {
        if let Some(addr) = args.get(pos + 1) {
            config.treasury_address = addr.clone();
        }
    }

    config
}

/// Result of the synchronous (in-tick) respawn authorization decision.
///
/// The sync decision never touches the network. When on-chain state must be
/// consulted, it returns a `Requires*` variant and the server runs the
/// corresponding async check — FixedUpdate is never blocked on RPC.
#[derive(Debug)]
pub enum RespawnAuth {
    /// Respawn authorized (free dev mode).
    Authorized,
    /// Respawn denied — wallet not verified.
    WalletNotVerified,
    /// BalanceGate mode + verified wallet: on-chain balance must be checked
    /// asynchronously before authorizing.
    RequiresChainCheck {
        /// The verified wallet address (base58) to check.
        wallet: String,
    },
    /// Paid mode + verified wallet: a client-signed treasury payment must be
    /// received and verified on-chain before authorizing.
    RequiresPayment {
        /// The verified wallet address (base58) the payment must come from.
        wallet: String,
    },
}

/// Synchronous part of the respawn authorization decision.
///
/// - `PaymentMode::Off` → always authorized (dev mode)
/// - wallet not verified → denied
/// - `BalanceGate` → caller runs the async balance check
/// - `Paid` → caller awaits + verifies a client-signed treasury payment
pub fn check_respawn_authorization(
    config: &RespawnConfig,
    client_id: u64,
    verified_wallets: &crate::auth::VerifiedWallets,
) -> RespawnAuth {
    // Dev mode: always allow
    if config.payment_mode == PaymentMode::Off {
        return RespawnAuth::Authorized;
    }

    // Gated modes: wallet must be verified first
    if !verified_wallets.is_verified(client_id) {
        return RespawnAuth::WalletNotVerified;
    }

    let wallet = verified_wallets
        .get_address(client_id)
        .expect("is_verified checked above")
        .to_string();
    match config.payment_mode {
        PaymentMode::Off => unreachable!("handled above"),
        PaymentMode::BalanceGate => RespawnAuth::RequiresChainCheck { wallet },
        PaymentMode::Paid => RespawnAuth::RequiresPayment { wallet },
    }
}

// ========================================
// On-chain verification (server-as-verifier)
// ========================================
//
// Architecture decision (CEO-approved): the game server NEVER signs
// transactions. Clients pay with their own wallets; the server only VERIFIES
// on-chain state via thin JSON-RPC calls (getBalance, getTransaction).
// This keeps the heavy solana-sdk dependency tree out of the game entirely.
//
// BalanceGate mode: respawn requires the verified wallet to hold at least
// `respawn_cost_lamports` on the configured cluster (devnet in staging).
// Paid mode: verify an actual client-signed payment to the treasury.

/// Thin chain access, abstracted so game logic is testable headless with a
/// mock while production uses JSON-RPC. Balance methods gate BalanceGate
/// mode; the transaction methods carry Paid mode (client submits, server
/// verifies). Unimplemented methods default to `Err` — mocks implement only
/// what their test consumes (fail closed by construction).
pub trait ChainRpc: Send + Sync + 'static {
    /// Return the lamport balance of `address`, or an error string.
    fn get_balance(&self, address: &str) -> Result<u64, String>;

    /// Latest blockhash for building a transaction (client-side pay flow).
    fn get_latest_blockhash(&self) -> Result<[u8; 32], String> {
        Err("get_latest_blockhash not supported by this provider".to_string())
    }

    /// Submit a base64-serialized signed transaction; returns its signature
    /// (client-side pay flow).
    fn send_transaction_base64(&self, _tx_base64: &str) -> Result<String, String> {
        Err("send_transaction_base64 not supported by this provider".to_string())
    }

    /// Fetch a confirmed transaction as jsonParsed JSON (server-side
    /// verification). `Ok(None)` = not found / not yet confirmed.
    fn get_transaction_json(&self, _signature: &str) -> Result<Option<serde_json::Value>, String> {
        Err("get_transaction_json not supported by this provider".to_string())
    }
}

/// Production provider: Solana JSON-RPC over HTTP.
/// Uses a blocking reqwest client — always called from a worker thread
/// (see `spawn_balance_check` / `spawn_payment_verification`), never from
/// the game loop.
pub struct JsonRpcChain {
    rpc_url: String,
    client: reqwest::blocking::Client,
}

impl JsonRpcChain {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
            client: reqwest::blocking::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// POST one JSON-RPC call and return the raw response value.
    fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        self.client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .map_err(|e| format!("rpc request failed: {e}"))?
            .json()
            .map_err(|e| format!("rpc response not json: {e}"))
    }
}

impl ChainRpc for JsonRpcChain {
    fn get_balance(&self, address: &str) -> Result<u64, String> {
        let response = self.call(
            "getBalance",
            serde_json::json!([address, {"commitment": "confirmed"}]),
        )?;
        parse_get_balance_response(&response)
    }

    fn get_latest_blockhash(&self) -> Result<[u8; 32], String> {
        let response = self.call(
            "getLatestBlockhash",
            serde_json::json!([{"commitment": "confirmed"}]),
        )?;
        if let Some(err) = response.get("error") {
            return Err(format!("rpc error: {err}"));
        }
        let hash_str = response
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.get("blockhash"))
            .and_then(|b| b.as_str())
            .ok_or_else(|| format!("malformed getLatestBlockhash response: {response}"))?;
        let bytes = bs58::decode(hash_str)
            .into_vec()
            .map_err(|e| format!("blockhash not base58: {e}"))?;
        bytes
            .try_into()
            .map_err(|_| "blockhash not 32 bytes".to_string())
    }

    fn send_transaction_base64(&self, tx_base64: &str) -> Result<String, String> {
        let response = self.call(
            "sendTransaction",
            serde_json::json!([tx_base64, {"encoding": "base64", "preflightCommitment": "confirmed"}]),
        )?;
        if let Some(err) = response.get("error") {
            return Err(format!("rpc error: {err}"));
        }
        response
            .get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("malformed sendTransaction response: {response}"))
    }

    fn get_transaction_json(&self, signature: &str) -> Result<Option<serde_json::Value>, String> {
        let response = self.call(
            "getTransaction",
            serde_json::json!([
                signature,
                {"encoding": "jsonParsed", "commitment": "confirmed", "maxSupportedTransactionVersion": 0}
            ]),
        )?;
        if let Some(err) = response.get("error") {
            return Err(format!("rpc error: {err}"));
        }
        match response.get("result") {
            None => Err(format!("malformed getTransaction response: {response}")),
            Some(serde_json::Value::Null) => Ok(None),
            Some(result) => Ok(Some(result.clone())),
        }
    }
}

/// Parse a `getBalance` JSON-RPC response into lamports.
/// Split out of the provider for direct unit testing.
pub fn parse_get_balance_response(response: &serde_json::Value) -> Result<u64, String> {
    if let Some(err) = response.get("error") {
        return Err(format!("rpc error: {err}"));
    }
    response
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_u64())
        .ok_or_else(|| format!("malformed getBalance response: {response}"))
}

/// Bevy resource wrapping the balance provider used by the respawn gate.
/// Production inserts `JsonRpcChain`; tests insert a mock.
#[derive(Resource, Clone)]
pub struct ChainVerifier(pub std::sync::Arc<dyn ChainRpc>);

impl ChainVerifier {
    pub fn json_rpc(rpc_url: &str) -> Self {
        Self(std::sync::Arc::new(JsonRpcChain::new(rpc_url)))
    }
}

/// Final verdict of an async on-chain check.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainCheckResult {
    /// Wallet holds at least the required lamports.
    Funded { available_lamports: u64 },
    /// Wallet balance is below the required lamports.
    InsufficientFunds {
        required_lamports: u64,
        available_lamports: u64,
    },
    /// RPC failed. The gate FAILS CLOSED: the player stays dead and the
    /// check is retried — an outage must never grant free respawns.
    RpcError(String),
}

/// Run a balance check on a worker thread; the receiver completes with the
/// verdict. The game loop polls with `try_recv` — never blocks.
pub fn spawn_balance_check(
    provider: std::sync::Arc<dyn ChainRpc>,
    wallet: String,
    required_lamports: u64,
) -> std::sync::mpsc::Receiver<ChainCheckResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = match provider.get_balance(&wallet) {
            Ok(available_lamports) if available_lamports >= required_lamports => {
                ChainCheckResult::Funded { available_lamports }
            }
            Ok(available_lamports) => ChainCheckResult::InsufficientFunds {
                required_lamports,
                available_lamports,
            },
            Err(e) => ChainCheckResult::RpcError(e),
        };
        // Receiver may have been dropped (player disconnected) — ignore.
        let _ = tx.send(result);
    });
    rx
}

// ========================================
// Pay-per-respawn: transaction building (client side)
// ========================================
//
// The client identity keypair IS the wallet (see auth.rs), so the client
// builds, signs, and submits its own payment transaction. We hand-roll the
// legacy Solana transaction wire format (~150 lines) instead of pulling in
// solana-sdk — same dependency argument as the server side, and the format
// is stable and small: transfer + memo is all we need.

/// System program id (native SOL transfers).
pub const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
/// SPL Memo program id.
pub const MEMO_PROGRAM_ID: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

/// The memo string that binds a payment to one death of one client.
pub fn respawn_memo(client_id: u64, nonce: u64) -> String {
    format!("ANIMA_PAY_v1:{client_id}:{nonce}")
}

/// Solana "shortvec" (compact-u16) length encoding.
fn encode_shortvec_len(out: &mut Vec<u8>, mut len: u16) {
    loop {
        let mut byte = (len & 0x7f) as u8;
        len >>= 7;
        if len != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
}

fn decode_base58_pubkey(address: &str) -> Result<[u8; 32], String> {
    let bytes = bs58::decode(address)
        .into_vec()
        .map_err(|e| format!("address '{address}' not base58: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| format!("address '{address}' not 32 bytes"))
}

/// Build and sign a legacy Solana transaction: SOL transfer to the treasury
/// plus a memo instruction. Returns the serialized signed transaction bytes
/// (ready for base64 sendTransaction).
///
/// Account layout (legacy message):
///   0: payer      (signer, writable)
///   1: treasury   (writable)
///   2: system program (readonly)
///   3: memo program   (readonly)
pub fn build_payment_transaction(
    signing_key: &ed25519_dalek::SigningKey,
    treasury: &str,
    lamports: u64,
    memo: &str,
    recent_blockhash: [u8; 32],
) -> Result<Vec<u8>, String> {
    use ed25519_dalek::Signer;

    let payer: [u8; 32] = signing_key.verifying_key().to_bytes();
    let treasury_key = decode_base58_pubkey(treasury)?;
    if treasury_key == payer {
        return Err("treasury must differ from payer".to_string());
    }
    let system_key = decode_base58_pubkey(SYSTEM_PROGRAM_ID)?;
    let memo_key = decode_base58_pubkey(MEMO_PROGRAM_ID)?;

    // --- Message ---
    let mut msg: Vec<u8> = Vec::with_capacity(256);
    // Header: 1 required signature, 0 readonly signed, 2 readonly unsigned.
    msg.push(1);
    msg.push(0);
    msg.push(2);
    // Account keys
    encode_shortvec_len(&mut msg, 4);
    msg.extend_from_slice(&payer);
    msg.extend_from_slice(&treasury_key);
    msg.extend_from_slice(&system_key);
    msg.extend_from_slice(&memo_key);
    // Recent blockhash
    msg.extend_from_slice(&recent_blockhash);
    // Instructions
    encode_shortvec_len(&mut msg, 2);
    // 1) SystemProgram::Transfer { lamports }
    msg.push(2); // program_id_index -> system program
    encode_shortvec_len(&mut msg, 2); // account indices: [payer, treasury]
    msg.push(0);
    msg.push(1);
    let mut transfer_data = Vec::with_capacity(12);
    transfer_data.extend_from_slice(&2u32.to_le_bytes()); // Transfer discriminant
    transfer_data.extend_from_slice(&lamports.to_le_bytes());
    encode_shortvec_len(&mut msg, transfer_data.len() as u16);
    msg.extend_from_slice(&transfer_data);
    // 2) Memo (no accounts; the memo body carries the nonce binding)
    msg.push(3); // program_id_index -> memo program
    encode_shortvec_len(&mut msg, 0);
    let memo_bytes = memo.as_bytes();
    encode_shortvec_len(&mut msg, memo_bytes.len() as u16);
    msg.extend_from_slice(memo_bytes);

    // --- Signature over the message ---
    let signature = signing_key.sign(&msg);

    // --- Transaction = signatures ++ message ---
    let mut tx: Vec<u8> = Vec::with_capacity(1 + 64 + msg.len());
    encode_shortvec_len(&mut tx, 1);
    tx.extend_from_slice(&signature.to_bytes());
    tx.extend_from_slice(&msg);
    Ok(tx)
}

/// Client-side pay flow, off-thread: fetch blockhash, build + sign + submit
/// the payment transaction. Receiver completes with the tx signature (base58)
/// to send to the server as `RespawnPaymentProof`.
pub fn spawn_payment_submission(
    rpc: std::sync::Arc<dyn ChainRpc>,
    signing_key: ed25519_dalek::SigningKey,
    treasury: String,
    lamports: u64,
    memo: String,
) -> std::sync::mpsc::Receiver<Result<String, String>> {
    use base64::Engine;
    let (tx_out, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let blockhash = rpc.get_latest_blockhash()?;
            let tx = build_payment_transaction(&signing_key, &treasury, lamports, &memo, blockhash)?;
            let tx_base64 = base64::engine::general_purpose::STANDARD.encode(&tx);
            rpc.send_transaction_base64(&tx_base64)
        })();
        let _ = tx_out.send(result);
    });
    rx
}

// ========================================
// Pay-per-respawn: transaction verification (server side)
// ========================================

/// What a respawn payment must look like on-chain to be accepted.
#[derive(Debug, Clone)]
pub struct PaymentExpectation {
    /// The auth-verified wallet the payment must be signed by.
    pub payer: String,
    /// Treasury address that must receive the lamports.
    pub treasury: String,
    /// Minimum lamports the treasury must have received.
    pub min_lamports: u64,
    /// Exact memo (see `respawn_memo`) binding the payment to this death.
    pub memo: String,
    /// Maximum age of the transaction (seconds vs blockTime). CEO decision:
    /// aligned with the blockhash validity window the chain enforces anyway.
    pub max_age_secs: i64,
}

/// Verdict on a submitted payment proof.
#[derive(Debug, Clone, PartialEq)]
pub enum PaymentVerdict {
    /// Payment verified on-chain — authorize the respawn.
    Verified,
    /// Transaction not found / not yet confirmed — poll again.
    NotFound,
    /// Definitively invalid — reject, do NOT honor.
    Rejected(String),
    /// RPC failed — fail closed and retry.
    RpcError(String),
}

/// Validate a jsonParsed `getTransaction` result against the expectation.
/// Pure function — unit-tested against canned RPC fixtures.
///
/// Checks, in order:
/// 1. transaction succeeded on-chain (`meta.err == null`)
/// 2. freshness: `blockTime` within `max_age_secs` of `now_unix`
/// 3. the fee payer (first signer) is the auth-verified wallet
/// 4. the treasury's balance increased by at least `min_lamports`
///    (pre/post balance delta — robust to instruction shape)
/// 5. the memo instruction carries the exact expected nonce memo
pub fn verify_payment_transaction(
    tx: &serde_json::Value,
    expect: &PaymentExpectation,
    now_unix: i64,
) -> PaymentVerdict {
    // 1. On-chain success
    let meta = match tx.get("meta") {
        Some(m) if !m.is_null() => m,
        _ => return PaymentVerdict::Rejected("transaction has no meta".to_string()),
    };
    if !meta.get("err").map(|e| e.is_null()).unwrap_or(false) {
        return PaymentVerdict::Rejected("transaction failed on-chain".to_string());
    }

    // 2. Freshness
    let Some(block_time) = tx.get("blockTime").and_then(|b| b.as_i64()) else {
        return PaymentVerdict::Rejected("transaction has no blockTime".to_string());
    };
    if (now_unix - block_time).abs() > expect.max_age_secs {
        return PaymentVerdict::Rejected(format!(
            "transaction too old ({}s > {}s window)",
            now_unix - block_time,
            expect.max_age_secs
        ));
    }

    // 3. Payer identity
    let Some(account_keys) = tx
        .pointer("/transaction/message/accountKeys")
        .and_then(|k| k.as_array())
    else {
        return PaymentVerdict::Rejected("transaction has no accountKeys".to_string());
    };
    let payer_ok = account_keys.first().is_some_and(|k| {
        k.get("pubkey").and_then(|p| p.as_str()) == Some(expect.payer.as_str())
            && k.get("signer").and_then(|s| s.as_bool()) == Some(true)
    });
    if !payer_ok {
        return PaymentVerdict::Rejected(format!(
            "payer is not the verified wallet {}",
            expect.payer
        ));
    }

    // 4. Treasury received the funds
    let Some(treasury_index) = account_keys.iter().position(|k| {
        k.get("pubkey").and_then(|p| p.as_str()) == Some(expect.treasury.as_str())
    }) else {
        return PaymentVerdict::Rejected("treasury not in transaction".to_string());
    };
    let pre = meta
        .get("preBalances")
        .and_then(|b| b.as_array())
        .and_then(|b| b.get(treasury_index))
        .and_then(|v| v.as_u64());
    let post = meta
        .get("postBalances")
        .and_then(|b| b.as_array())
        .and_then(|b| b.get(treasury_index))
        .and_then(|v| v.as_u64());
    let (Some(pre), Some(post)) = (pre, post) else {
        return PaymentVerdict::Rejected("missing treasury balances".to_string());
    };
    let received = post.saturating_sub(pre);
    if received < expect.min_lamports {
        return PaymentVerdict::Rejected(format!(
            "treasury received {received} lamports, expected >= {}",
            expect.min_lamports
        ));
    }

    // 5. Memo nonce binding (anti-replay)
    let memo_ok = tx
        .pointer("/transaction/message/instructions")
        .and_then(|i| i.as_array())
        .is_some_and(|instructions| {
            instructions.iter().any(|ix| {
                ix.get("program").and_then(|p| p.as_str()) == Some("spl-memo")
                    && ix.get("parsed").and_then(|p| p.as_str()) == Some(expect.memo.as_str())
            })
        });
    if !memo_ok {
        return PaymentVerdict::Rejected(format!("memo does not match '{}'", expect.memo));
    }

    PaymentVerdict::Verified
}

/// Server-side verification worker: poll `getTransaction` for the proof's
/// signature until it confirms (or the deadline passes), then validate it
/// against the expectation. Receiver completes with the final verdict.
pub fn spawn_payment_verification(
    rpc: std::sync::Arc<dyn ChainRpc>,
    signature: String,
    expect: PaymentExpectation,
    poll_interval: std::time::Duration,
    deadline: std::time::Duration,
) -> std::sync::mpsc::Receiver<PaymentVerdict> {
    let (tx_out, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let verdict = loop {
            match rpc.get_transaction_json(&signature) {
                Ok(Some(tx)) => {
                    let now_unix = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    break verify_payment_transaction(&tx, &expect, now_unix);
                }
                Ok(None) => {
                    if started.elapsed() > deadline {
                        break PaymentVerdict::Rejected(
                            "payment not confirmed before deadline".to_string(),
                        );
                    }
                    std::thread::sleep(poll_interval);
                }
                Err(e) => break PaymentVerdict::RpcError(e),
            }
        };
        let _ = tx_out.send(verdict);
    });
    rx
}

// ========================================
// Pay-per-respawn: server-side payment ledger
// ========================================

/// Tracks outstanding payment nonces and consumed transaction signatures.
///
/// DEVNET ONLY — both maps are in-memory (CEO-acked):
/// - On server restart, pending nonces are lost: a player who paid but whose
///   proof wasn't verified yet will be REJECTED (their memo no longer matches
///   any outstanding nonce) — the restart-double-charge case. Nonces are
///   seeded from wall-clock time so stale memos can never match new nonces.
/// - Consumed signatures are also lost, but nonce freshness + the tx
///   freshness window bound the replay surface.
///
/// MAINNET BLOCKER (tracked in 543fbc05/chain-status): a persistent
/// append-only consumed-signature log. That solves both replay and restart
/// recovery: after a restart, a proof with an unknown nonce but an
/// otherwise-valid fresh transaction and unconsumed signature can be honored
/// safely instead of double-charging the player.
#[derive(Resource)]
pub struct PaymentLedger {
    /// client_id → nonce issued for the player's current death.
    pub outstanding: std::collections::HashMap<u64, u64>,
    /// Verified-and-spent transaction signatures (single-use).
    pub consumed_signatures: std::collections::HashSet<String>,
    next_nonce: u64,
}

impl Default for PaymentLedger {
    fn default() -> Self {
        // Seed nonces from wall-clock so memos from before a restart can
        // never match a nonce issued after it.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            << 20;
        Self {
            outstanding: std::collections::HashMap::new(),
            consumed_signatures: std::collections::HashSet::new(),
            next_nonce: seed,
        }
    }
}

impl PaymentLedger {
    /// Issue a fresh nonce for a player's death, replacing any prior one.
    pub fn issue_nonce(&mut self, client_id: u64) -> u64 {
        self.next_nonce += 1;
        let nonce = self.next_nonce;
        self.outstanding.insert(client_id, nonce);
        nonce
    }

    /// The player's outstanding nonce, if a payment request is live.
    pub fn outstanding_nonce(&self, client_id: u64) -> Option<u64> {
        self.outstanding.get(&client_id).copied()
    }

    /// Consume the nonce + signature after a verified payment.
    /// Returns false if the signature was already consumed (replay).
    pub fn consume(&mut self, client_id: u64, signature: &str) -> bool {
        if !self.consumed_signatures.insert(signature.to_string()) {
            return false;
        }
        self.outstanding.remove(&client_id);
        true
    }

    /// Drop a player's outstanding nonce (disconnect / respawn).
    pub fn clear(&mut self, client_id: u64) {
        self.outstanding.remove(&client_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::VerifiedWallets;

    fn balance_config() -> RespawnConfig {
        RespawnConfig {
            payment_mode: PaymentMode::BalanceGate,
            ..RespawnConfig::default()
        }
    }

    fn paid_config() -> RespawnConfig {
        RespawnConfig {
            payment_mode: PaymentMode::Paid,
            ..RespawnConfig::default()
        }
    }

    fn verified(client_id: u64) -> VerifiedWallets {
        let mut vw = VerifiedWallets::default();
        vw.wallets
            .insert(client_id, "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string());
        vw
    }

    #[test]
    fn dev_mode_always_authorizes_even_unverified() {
        let config = RespawnConfig::default();
        assert_eq!(config.payment_mode, PaymentMode::Off, "default config must be free-respawn dev mode");
        let result = check_respawn_authorization(&config, 42, &VerifiedWallets::default());
        assert!(matches!(result, RespawnAuth::Authorized));
    }

    #[test]
    fn gated_modes_deny_unverified_wallet() {
        for config in [balance_config(), paid_config()] {
            let result = check_respawn_authorization(&config, 42, &VerifiedWallets::default());
            assert!(
                matches!(result, RespawnAuth::WalletNotVerified),
                "{:?} must deny unverified wallets",
                config.payment_mode
            );
        }
    }

    #[test]
    fn gated_modes_deny_other_clients_verification() {
        // Client 99 is verified; client 42 must not ride on it.
        for config in [balance_config(), paid_config()] {
            let result = check_respawn_authorization(&config, 42, &verified(99));
            assert!(matches!(result, RespawnAuth::WalletNotVerified));
        }
    }

    /// The stub is gone: balance mode + verified wallet no longer authorizes
    /// blindly — it demands an async on-chain check for the verified wallet.
    #[test]
    fn balance_mode_verified_wallet_requires_chain_check() {
        let result = check_respawn_authorization(&balance_config(), 42, &verified(42));
        match result {
            RespawnAuth::RequiresChainCheck { wallet } => {
                assert_eq!(wallet, "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU");
            }
            other => panic!("expected RequiresChainCheck, got {other:?}"),
        }
    }

    /// Paid mode + verified wallet: a client-signed treasury payment is
    /// demanded — holding a balance is not enough.
    #[test]
    fn paid_mode_verified_wallet_requires_payment() {
        let result = check_respawn_authorization(&paid_config(), 42, &verified(42));
        match result {
            RespawnAuth::RequiresPayment { wallet } => {
                assert_eq!(wallet, "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU");
            }
            other => panic!("expected RequiresPayment, got {other:?}"),
        }
    }

    #[test]
    fn payment_mode_parses() {
        assert_eq!(PaymentMode::parse("off"), Some(PaymentMode::Off));
        assert_eq!(PaymentMode::parse("Balance"), Some(PaymentMode::BalanceGate));
        assert_eq!(PaymentMode::parse("PAID"), Some(PaymentMode::Paid));
        assert_eq!(PaymentMode::parse("mainnet"), None);
    }

    // ---- Async balance check ----

    struct MockBalances(std::collections::HashMap<String, u64>);
    impl ChainRpc for MockBalances {
        fn get_balance(&self, address: &str) -> Result<u64, String> {
            self.0
                .get(address)
                .copied()
                .ok_or_else(|| "unknown address".to_string())
        }
    }

    fn mock_provider(address: &str, lamports: u64) -> std::sync::Arc<dyn ChainRpc> {
        let mut m = std::collections::HashMap::new();
        m.insert(address.to_string(), lamports);
        std::sync::Arc::new(MockBalances(m))
    }

    const WALLET: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    fn recv_verdict(rx: std::sync::mpsc::Receiver<ChainCheckResult>) -> ChainCheckResult {
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("balance check thread must complete")
    }

    #[test]
    fn balance_check_funded_wallet_is_funded() {
        let rx = spawn_balance_check(mock_provider(WALLET, 20_000_000), WALLET.into(), 10_000_000);
        assert_eq!(
            recv_verdict(rx),
            ChainCheckResult::Funded { available_lamports: 20_000_000 }
        );
    }

    #[test]
    fn balance_check_exact_balance_is_funded() {
        let rx = spawn_balance_check(mock_provider(WALLET, 10_000_000), WALLET.into(), 10_000_000);
        assert_eq!(
            recv_verdict(rx),
            ChainCheckResult::Funded { available_lamports: 10_000_000 }
        );
    }

    #[test]
    fn balance_check_underfunded_wallet_is_insufficient() {
        let rx = spawn_balance_check(mock_provider(WALLET, 9_999_999), WALLET.into(), 10_000_000);
        assert_eq!(
            recv_verdict(rx),
            ChainCheckResult::InsufficientFunds {
                required_lamports: 10_000_000,
                available_lamports: 9_999_999,
            }
        );
    }

    #[test]
    fn balance_check_rpc_failure_fails_closed() {
        let rx = spawn_balance_check(mock_provider(WALLET, 0), "SomeOtherWallet".into(), 1);
        assert!(matches!(recv_verdict(rx), ChainCheckResult::RpcError(_)));
    }

    // ---- getBalance response parsing ----

    #[test]
    fn parse_get_balance_ok() {
        let response = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"context": {"slot": 12345}, "value": 123456789u64},
        });
        assert_eq!(parse_get_balance_response(&response), Ok(123_456_789));
    }

    #[test]
    fn parse_get_balance_rpc_error() {
        let response = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "error": {"code": -32602, "message": "Invalid param: WrongSize"},
        });
        assert!(parse_get_balance_response(&response).unwrap_err().contains("rpc error"));
    }

    #[test]
    fn parse_get_balance_malformed() {
        let response = serde_json::json!({"jsonrpc": "2.0", "id": 1});
        assert!(parse_get_balance_response(&response).unwrap_err().contains("malformed"));
    }

    // ---- Transaction serializer ----

    #[test]
    fn shortvec_encoding_vectors() {
        for (len, expected) in [
            (0u16, vec![0x00]),
            (1, vec![0x01]),
            (127, vec![0x7f]),
            (128, vec![0x80, 0x01]),
            (255, vec![0xff, 0x01]),
            (16384, vec![0x80, 0x80, 0x01]),
        ] {
            let mut out = Vec::new();
            encode_shortvec_len(&mut out, len);
            assert_eq!(out, expected, "shortvec({len})");
        }
    }

    /// Field-level teardown of a built transaction: byte offsets, header,
    /// account ordering, instruction encoding, and a valid signature over
    /// the exact message bytes.
    #[test]
    fn payment_transaction_structure_and_signature() {
        use ed25519_dalek::Verifier;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let payer = signing_key.verifying_key().to_bytes();
        let treasury = "GsmLNe7R7rrebLdSLwdebd361WZmZpp4JTzzR6pbuK1X";
        let blockhash = [9u8; 32];
        let memo = respawn_memo(42, 1234);

        let tx = build_payment_transaction(&signing_key, treasury, 10_000_000, &memo, blockhash)
            .expect("build must succeed");

        // [0] signature count, [1..65] signature, [65..] message
        assert_eq!(tx[0], 1, "one signature");
        let sig_bytes: [u8; 64] = tx[1..65].try_into().unwrap();
        let msg = &tx[65..];

        // Signature verifies over the message bytes with the payer key.
        signing_key
            .verifying_key()
            .verify(msg, &ed25519_dalek::Signature::from_bytes(&sig_bytes))
            .expect("signature must verify over message bytes");

        // Header
        assert_eq!(&msg[0..3], &[1, 0, 2], "1 signer, 0 ro-signed, 2 ro-unsigned");
        // Account keys: count then 4 × 32 bytes in fixed order
        assert_eq!(msg[3], 4);
        assert_eq!(&msg[4..36], &payer);
        assert_eq!(&msg[36..68], decode_base58_pubkey(treasury).unwrap().as_slice());
        assert_eq!(&msg[68..100], decode_base58_pubkey(SYSTEM_PROGRAM_ID).unwrap().as_slice());
        assert_eq!(&msg[100..132], decode_base58_pubkey(MEMO_PROGRAM_ID).unwrap().as_slice());
        // Blockhash
        assert_eq!(&msg[132..164], &blockhash);
        // Instructions: 2. Transfer: program 2, accounts [0,1], 12-byte data
        assert_eq!(msg[164], 2, "two instructions");
        assert_eq!(&msg[165..168], &[2, 2, 0], "transfer: sys program, 2 accounts, [0..");
        assert_eq!(msg[168], 1);
        assert_eq!(msg[169], 12, "transfer data length");
        assert_eq!(&msg[170..174], &2u32.to_le_bytes(), "Transfer discriminant");
        assert_eq!(&msg[174..182], &10_000_000u64.to_le_bytes(), "lamports");
        // Memo: program 3, 0 accounts, memo bytes
        assert_eq!(msg[182], 3, "memo program index");
        assert_eq!(msg[183], 0, "memo has no accounts");
        assert_eq!(msg[184] as usize, memo.len());
        assert_eq!(&msg[185..185 + memo.len()], memo.as_bytes());
        assert_eq!(msg.len(), 185 + memo.len(), "no trailing bytes");
    }

    #[test]
    fn payment_transaction_rejects_paying_self() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let self_address = crate::auth::pubkey_address(&signing_key.verifying_key().to_bytes());
        let result =
            build_payment_transaction(&signing_key, &self_address, 1, "m", [0u8; 32]);
        assert!(result.is_err(), "paying yourself must be rejected");
    }

    // ---- Payment verification ----

    const TREASURY: &str = "GsmLNe7R7rrebLdSLwdebd361WZmZpp4JTzzR6pbuK1X";

    fn expectation() -> PaymentExpectation {
        PaymentExpectation {
            payer: WALLET.to_string(),
            treasury: TREASURY.to_string(),
            min_lamports: 10_000_000,
            memo: respawn_memo(42, 1234),
            max_age_secs: 90,
        }
    }

    /// A canned jsonParsed getTransaction result for a valid payment.
    fn valid_tx_json(now: i64) -> serde_json::Value {
        serde_json::json!({
            "blockTime": now - 5,
            "slot": 123456,
            "meta": {
                "err": null,
                "preBalances": [700_000_000u64, 0u64, 1u64, 1u64],
                "postBalances": [689_995_000u64, 10_000_000u64, 1u64, 1u64],
            },
            "transaction": {
                "message": {
                    "accountKeys": [
                        {"pubkey": WALLET, "signer": true, "writable": true},
                        {"pubkey": TREASURY, "signer": false, "writable": true},
                        {"pubkey": SYSTEM_PROGRAM_ID, "signer": false, "writable": false},
                        {"pubkey": MEMO_PROGRAM_ID, "signer": false, "writable": false},
                    ],
                    "instructions": [
                        {
                            "program": "system",
                            "programId": SYSTEM_PROGRAM_ID,
                            "parsed": {
                                "type": "transfer",
                                "info": {"source": WALLET, "destination": TREASURY, "lamports": 10_000_000u64},
                            },
                        },
                        {
                            "program": "spl-memo",
                            "programId": MEMO_PROGRAM_ID,
                            "parsed": respawn_memo(42, 1234),
                        },
                    ],
                },
            },
        })
    }

    #[test]
    fn verify_accepts_valid_payment() {
        let now = 1_800_000_000;
        assert_eq!(
            verify_payment_transaction(&valid_tx_json(now), &expectation(), now),
            PaymentVerdict::Verified
        );
    }

    #[test]
    fn verify_rejects_failed_transaction() {
        let now = 1_800_000_000;
        let mut tx = valid_tx_json(now);
        tx["meta"]["err"] = serde_json::json!({"InstructionError": [0, "Custom"]});
        assert!(matches!(
            verify_payment_transaction(&tx, &expectation(), now),
            PaymentVerdict::Rejected(r) if r.contains("failed on-chain")
        ));
    }

    #[test]
    fn verify_rejects_stale_transaction() {
        let now = 1_800_000_000;
        let mut tx = valid_tx_json(now);
        tx["blockTime"] = serde_json::json!(now - 300);
        assert!(matches!(
            verify_payment_transaction(&tx, &expectation(), now),
            PaymentVerdict::Rejected(r) if r.contains("too old")
        ));
    }

    #[test]
    fn verify_rejects_wrong_payer() {
        let now = 1_800_000_000;
        let mut tx = valid_tx_json(now);
        // Someone else signed the payment — even to the right treasury with
        // the right memo, it must not count for OUR player's wallet.
        tx["transaction"]["message"]["accountKeys"][0]["pubkey"] =
            serde_json::json!("DaLpc2HCiC49vMrjagN9J7zJf1E4Ri9KJcRyXnxC7e1B");
        assert!(matches!(
            verify_payment_transaction(&tx, &expectation(), now),
            PaymentVerdict::Rejected(r) if r.contains("payer")
        ));
    }

    #[test]
    fn verify_rejects_underpayment() {
        let now = 1_800_000_000;
        let mut tx = valid_tx_json(now);
        tx["meta"]["postBalances"] = serde_json::json!([690_004_999u64, 9_999_999u64, 1u64, 1u64]);
        assert!(matches!(
            verify_payment_transaction(&tx, &expectation(), now),
            PaymentVerdict::Rejected(r) if r.contains("treasury received")
        ));
    }

    #[test]
    fn verify_rejects_missing_treasury() {
        let now = 1_800_000_000;
        let mut tx = valid_tx_json(now);
        tx["transaction"]["message"]["accountKeys"][1]["pubkey"] =
            serde_json::json!("DaLpc2HCiC49vMrjagN9J7zJf1E4Ri9KJcRyXnxC7e1B");
        assert!(matches!(
            verify_payment_transaction(&tx, &expectation(), now),
            PaymentVerdict::Rejected(r) if r.contains("treasury not in transaction")
        ));
    }

    #[test]
    fn verify_rejects_wrong_memo_nonce() {
        let now = 1_800_000_000;
        let mut tx = valid_tx_json(now);
        // Replayed payment from a previous death: right payer, right amount,
        // wrong (old) nonce in the memo.
        tx["transaction"]["message"]["instructions"][1]["parsed"] =
            serde_json::json!(respawn_memo(42, 1233));
        assert!(matches!(
            verify_payment_transaction(&tx, &expectation(), now),
            PaymentVerdict::Rejected(r) if r.contains("memo")
        ));
    }

    #[test]
    fn verify_rejects_missing_memo() {
        let now = 1_800_000_000;
        let mut tx = valid_tx_json(now);
        tx["transaction"]["message"]["instructions"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(matches!(
            verify_payment_transaction(&tx, &expectation(), now),
            PaymentVerdict::Rejected(r) if r.contains("memo")
        ));
    }

    // ---- Payment ledger ----

    #[test]
    fn ledger_nonces_are_single_use_and_replayproof() {
        let mut ledger = PaymentLedger::default();
        let n1 = ledger.issue_nonce(42);
        assert_eq!(ledger.outstanding_nonce(42), Some(n1));

        // Re-issue replaces (new death, new nonce).
        let n2 = ledger.issue_nonce(42);
        assert_ne!(n1, n2);
        assert_eq!(ledger.outstanding_nonce(42), Some(n2));

        // Consume clears the nonce and burns the signature.
        assert!(ledger.consume(42, "sig-1"));
        assert_eq!(ledger.outstanding_nonce(42), None);
        // Replaying the same signature fails, even for another client.
        assert!(!ledger.consume(42, "sig-1"));
        assert!(!ledger.consume(99, "sig-1"));

        // Nonces are strictly increasing across players.
        let n3 = ledger.issue_nonce(7);
        assert!(n3 > n2);
    }

    // ---- Live devnet tests (ignored by default) ----

    /// Live devnet smoke test — ignored by default (network). Run with:
    /// `cargo test --lib solana -- --ignored`
    #[test]
    #[ignore = "hits real devnet RPC"]
    fn devnet_get_balance_smoke() {
        let provider = JsonRpcChain::new("https://api.devnet.solana.com");
        // System program account always exists; balance query must succeed.
        let balance = provider
            .get_balance("11111111111111111111111111111111")
            .expect("devnet getBalance must succeed");
        // The system program account holds a nonzero lamport balance.
        assert!(balance > 0);
    }

    /// THE round-trip proof (CEO-required): build a real payment with the
    /// hand-rolled serializer, submit it to devnet, fetch it back with
    /// getTransaction, and require our own verifier to accept it.
    /// Spends real devnet lamports from ~/.anima/keypair.json (faucet money).
    #[test]
    #[ignore = "submits a real devnet transaction; needs faucet-funded ~/.anima/keypair.json"]
    fn devnet_payment_round_trip() {
        let rpc = std::sync::Arc::new(JsonRpcChain::new("https://api.devnet.solana.com"));
        let (signing_key, pubkey) = crate::auth::load_or_create_keypair(None);
        let payer = crate::auth::pubkey_address(&pubkey);
        let memo = respawn_memo(42, 999_999);
        let lamports = 10_000_000; // 0.01 SOL respawn cost

        // Build + submit through the exact client code path.
        let rx = spawn_payment_submission(
            rpc.clone(),
            signing_key,
            TREASURY.to_string(),
            lamports,
            memo.clone(),
        );
        let signature = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("submission thread must complete")
            .expect("devnet sendTransaction must accept our hand-rolled tx");

        // Verify through the exact server code path.
        let rx = spawn_payment_verification(
            rpc,
            signature.clone(),
            PaymentExpectation {
                payer,
                treasury: TREASURY.to_string(),
                min_lamports: lamports,
                memo,
                max_age_secs: 90,
            },
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(60),
        );
        let verdict = rx
            .recv_timeout(std::time::Duration::from_secs(90))
            .expect("verification thread must complete");
        assert_eq!(
            verdict,
            PaymentVerdict::Verified,
            "devnet round-trip (sig {signature}) must verify"
        );
    }
}

/// Replicated component: the player's verified Solana wallet address.
/// Attached to player entities after successful wallet auth verification.
/// Visible to all clients (for display in kill feed, scoreboard, etc).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct WalletAddress(pub String);
