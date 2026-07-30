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
// on-chain state via thin JSON-RPC calls (getBalance now,
// getSignatureStatuses when payment verification lands). This keeps the
// heavy solana-sdk dependency tree out of the game entirely.
//
// BalanceGate mode: respawn requires the verified wallet to hold at least
// `respawn_cost_lamports` on the configured cluster (devnet in staging).
// Paid mode: verify an actual client-signed payment to the treasury.

/// Source of on-chain balance data. Abstracted so the respawn gate is
/// testable headless with a mock, while production uses JSON-RPC.
pub trait BalanceProvider: Send + Sync + 'static {
    /// Return the lamport balance of `address`, or an error string.
    fn get_balance(&self, address: &str) -> Result<u64, String>;
}

/// Production provider: Solana JSON-RPC `getBalance` over HTTP.
/// Uses a blocking reqwest client — always called from a worker thread
/// (see `spawn_balance_check`), never from the game loop.
pub struct JsonRpcBalanceProvider {
    rpc_url: String,
    client: reqwest::blocking::Client,
}

impl JsonRpcBalanceProvider {
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
}

impl BalanceProvider for JsonRpcBalanceProvider {
    fn get_balance(&self, address: &str) -> Result<u64, String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getBalance",
            "params": [address, {"commitment": "confirmed"}],
        });
        let response: serde_json::Value = self
            .client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .map_err(|e| format!("rpc request failed: {e}"))?
            .json()
            .map_err(|e| format!("rpc response not json: {e}"))?;
        parse_get_balance_response(&response)
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
/// Production inserts `JsonRpcBalanceProvider`; tests insert a mock.
#[derive(Resource, Clone)]
pub struct ChainVerifier(pub std::sync::Arc<dyn BalanceProvider>);

impl ChainVerifier {
    pub fn json_rpc(rpc_url: &str) -> Self {
        Self(std::sync::Arc::new(JsonRpcBalanceProvider::new(rpc_url)))
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
    provider: std::sync::Arc<dyn BalanceProvider>,
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
    impl BalanceProvider for MockBalances {
        fn get_balance(&self, address: &str) -> Result<u64, String> {
            self.0
                .get(address)
                .copied()
                .ok_or_else(|| "unknown address".to_string())
        }
    }

    fn mock_provider(address: &str, lamports: u64) -> std::sync::Arc<dyn BalanceProvider> {
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

    /// Live devnet smoke test — ignored by default (network). Run with:
    /// `cargo test --lib solana -- --ignored`
    #[test]
    #[ignore = "hits real devnet RPC"]
    fn devnet_get_balance_smoke() {
        let provider = JsonRpcBalanceProvider::new("https://api.devnet.solana.com");
        // System program account always exists; balance query must succeed.
        let balance = provider
            .get_balance("11111111111111111111111111111111")
            .expect("devnet getBalance must succeed");
        // The system program account holds a nonzero lamport balance.
        assert!(balance > 0);
    }
}

/// Replicated component: the player's verified Solana wallet address.
/// Attached to player entities after successful wallet auth verification.
/// Visible to all clients (for display in kill feed, scoreboard, etc).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct WalletAddress(pub String);
