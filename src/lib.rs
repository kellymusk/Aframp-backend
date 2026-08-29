#![allow(non_snake_case)]
#![cfg_attr(not(feature = "database"), no_std)]

// Import soroban SDK items only when not using database feature
#[cfg(not(feature = "database"))]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, Env, String, Symbol, Vec,
};

// ── Core infrastructure ────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod database;

#[cfg(feature = "database")]
pub mod error;

#[cfg(feature = "database")]
pub mod config;

#[cfg(feature = "database")]
pub mod middleware;

#[cfg(feature = "database")]
pub mod logging;

#[cfg(feature = "database")]
pub mod telemetry;

#[cfg(feature = "database")]
pub mod health;

// ── Cache & persistence ────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod cache;

// ── Auth & identity ────────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod auth;

#[cfg(feature = "database")]
pub mod oauth;

#[cfg(feature = "database")]
pub mod api_keys;

#[cfg(feature = "database")]
pub mod crypto;

#[cfg(feature = "database")]
pub mod key_management;

// ── KYC / Verification ─────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod kyc;

#[cfg(feature = "database")]
pub mod verification;

// ── Payments & Onramp/Offramp core ────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod payments;

#[cfg(feature = "database")]
pub mod rate_engine;

#[cfg(feature = "database")]
pub mod corridors;

#[cfg(feature = "database")]
pub mod oracle;

#[cfg(feature = "database")]
pub mod banking;

#[cfg(feature = "database")]
pub mod recurring;

// ── Stellar / blockchain ───────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod stellar;

// ── Wallet ─────────────────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod wallet;

#[cfg(feature = "database")]
pub mod wallet_provisioning;

// ── Compliance & AML ──────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod aml;

#[cfg(feature = "database")]
pub mod sanctions;

#[cfg(feature = "database")]
pub mod compliance;

#[cfg(feature = "database")]
pub mod risk;

// ── API layer ─────────────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod api;

#[cfg(feature = "database")]
pub mod routes;

#[cfg(feature = "database")]
pub mod gateway;

// ── Admin ─────────────────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod admin;

// ── Background workers ────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod workers;

#[cfg(feature = "database")]
pub mod jobs;

#[cfg(feature = "database")]
pub mod event_bus;

// ── Observability ─────────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod metrics;

#[cfg(feature = "database")]
pub mod audit;

#[cfg(feature = "database")]
pub mod analytics;

#[cfg(feature = "database")]
pub mod reporting;

// ── Security ──────────────────────────────────────────────────────────────────
#[cfg(feature = "database")]
pub mod security;

#[cfg(feature = "database")]
pub mod masking;

// ── Services (email, notifications, etc.) ────────────────────────────────────
#[cfg(feature = "database")]
pub mod services;

// ── Multi-region — removed for basic setup ──────────────────────────────────
// pub mod multi_region;

// ── Adaptive rate limiting ────────────────────────────────────────────────────
// #[cfg(feature = "cache")]
// pub mod adaptive_rate_limit;  // removed — not needed for basic onramp/offramp

// ─────────────────────────────────────────────────────────────────────────────
// Soroban escrow contract (compiled when NOT using the database feature)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "database"))]
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidFeeRate = 4,
    ContractPaused = 5,
    OrderNotFound = 100,
    InvalidOrderStatus = 101,
    OrderExpired = 102,
    CannotAcceptOwnOrder = 103,
    TransferFailed = 104,
}

#[cfg(not(feature = "database"))]
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrderStatus {
    Open,
    Locked,
    PaymentSent,
    Completed,
    Disputed,
    Cancelled,
}

#[cfg(not(feature = "database"))]
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Order {
    pub id: u64,
    pub seller: Address,
    pub buyer: Option<Address>,
    pub token: Address,
    pub amount: i128,
    pub fiat_currency: Symbol,
    pub fiat_amount: i128,
    pub rate: i128,
    pub status: OrderStatus,
    pub created_at: u64,
    pub expires_at: u64,
    pub payment_method: String,
}

#[cfg(not(feature = "database"))]
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    OrderCount,
    Order(u64),
    UserOrders(Address),
    FeeRate,
    FeeTreasury,
    IsPaused,
    DisputeResolver,
}

#[cfg(not(feature = "database"))]
#[contract]
pub struct EscrowContract;

#[cfg(not(feature = "database"))]
#[contractimpl]
impl EscrowContract {
    pub fn initialize(
        env: Env,
        admin: Address,
        fee_rate: u32,
        fee_treasury: Address,
        dispute_resolver: Address,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        if fee_rate > 1000 {
            return Err(Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::FeeRate, &fee_rate);
        env.storage().instance().set(&DataKey::FeeTreasury, &fee_treasury);
        env.storage().instance().set(&DataKey::DisputeResolver, &dispute_resolver);
        env.storage().instance().set(&DataKey::IsPaused, &false);
        env.storage().instance().set(&DataKey::OrderCount, &0u64);
        Ok(())
    }

    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        Ok(())
    }

    pub fn set_fee_rate(env: Env, new_fee_rate: u32) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        if new_fee_rate > 1000 {
            return Err(Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::FeeRate, &new_fee_rate);
        Ok(())
    }

    pub fn pause(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &true);
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &false);
        Ok(())
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage().instance().get(&DataKey::IsPaused).unwrap_or(false)
    }

    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)
    }
}
