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

/// Configuration for the pay-to-respawn system.
///
/// In dev mode, `require_payment` is false and all respawns are free.
/// In production, the server checks the player's Solana wallet for:
/// 1. ANIMA_RESPAWN tokens (burned on use)
/// 2. Sufficient SOL balance (transferred to treasury)
///
/// The `respawn_cost_lamports` field sets the SOL cost if no respawn tokens
/// are available. 1 SOL = 1_000_000_000 lamports.
#[derive(Resource, Clone, Debug)]
pub struct RespawnConfig {
    /// Whether respawn requires payment. False in dev, true in production.
    pub require_payment: bool,

    /// SOL cost to respawn (in lamports) if no RESPAWN tokens available.
    /// Default: 10_000_000 (0.01 SOL)
    pub respawn_cost_lamports: u64,

    /// Solana RPC endpoint for balance checks.
    /// Default: "http://localhost:8899" (local validator)
    pub rpc_url: String,

    /// Treasury wallet address that receives respawn SOL fees.
    pub treasury_address: String,
}

impl Default for RespawnConfig {
    fn default() -> Self {
        Self {
            // Dev mode: respawns are free
            require_payment: false,
            // 0.01 SOL
            respawn_cost_lamports: 10_000_000,
            // Local validator
            rpc_url: "http://localhost:8899".to_string(),
            // Placeholder — set this to the actual treasury wallet in production
            treasury_address: "11111111111111111111111111111111".to_string(),
        }
    }
}

/// Parse --require-respawn-payment from CLI args.
pub fn parse_respawn_config() -> RespawnConfig {
    let args: Vec<String> = std::env::args().collect();
    let mut config = RespawnConfig::default();

    if args.contains(&"--require-respawn-payment".to_string()) {
        config.require_payment = true;
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

/// Result of a respawn authorization check.
#[derive(Debug)]
pub enum RespawnAuth {
    /// Respawn authorized (free mode or payment confirmed)
    Authorized,
    /// Respawn denied — insufficient funds
    InsufficientFunds {
        required_lamports: u64,
        available_lamports: u64,
    },
    /// Respawn denied — wallet not verified
    WalletNotVerified,
}

/// Check if a player is authorized to respawn.
///
/// Current implementation:
/// - If `require_payment` is false → always authorized (dev mode)
/// - If `require_payment` is true → check if wallet is verified
///
/// Future implementation (when Solana RPC is wired):
/// 1. Check ANIMA_RESPAWN token balance → burn 1 if available
/// 2. Check SOL balance → transfer respawn_cost_lamports to treasury
/// 3. Deny if neither condition met
pub fn check_respawn_authorization(
    config: &RespawnConfig,
    client_id: u64,
    verified_wallets: &crate::auth::VerifiedWallets,
) -> RespawnAuth {
    // Dev mode: always allow
    if !config.require_payment {
        return RespawnAuth::Authorized;
    }

    // Production mode: wallet must be verified first
    if !verified_wallets.is_verified(client_id) {
        return RespawnAuth::WalletNotVerified;
    }

    // TODO: Solana RPC balance check
    //
    // Future flow:
    // 1. let wallet_address = verified_wallets.get_address(client_id).unwrap();
    // 2. let rpc_client = solana_client::rpc_client::RpcClient::new(&config.rpc_url);
    //
    // Check RESPAWN token:
    // 3. let respawn_mint = Pubkey::from_str("ANIMA_RESPAWN_MINT_ADDRESS")?;
    // 4. let ata = get_associated_token_address(&wallet_pubkey, &respawn_mint);
    // 5. let balance = rpc_client.get_token_account_balance(&ata)?;
    // 6. if balance.amount > 0 {
    //        // Burn 1 respawn token
    //        let ix = spl_token::instruction::burn(...);
    //        // Submit transaction
    //        return RespawnAuth::Authorized;
    //    }
    //
    // Check SOL balance:
    // 7. let sol_balance = rpc_client.get_balance(&wallet_pubkey)?;
    // 8. if sol_balance >= config.respawn_cost_lamports {
    //        // Transfer to treasury
    //        let ix = system_instruction::transfer(...);
    //        return RespawnAuth::Authorized;
    //    }
    //
    // 9. return RespawnAuth::InsufficientFunds { ... };

    // For now: if wallet is verified, authorize
    // This is the hook point — swap this return for real Solana checks
    RespawnAuth::Authorized
}

/// Replicated component: the player's verified Solana wallet address.
/// Attached to player entities after successful wallet auth verification.
/// Visible to all clients (for display in kill feed, scoreboard, etc).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct WalletAddress(pub String);
