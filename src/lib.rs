#![allow(non_snake_case)]
#![cfg_attr(not(feature = "database"), no_std)]

// Import soroban SDK items only when not using database feature
#[cfg(not(feature = "database"))]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, Env, String, Symbol, Vec,
};

// Database module requires std and specific dependencies
#[cfg(feature = "database")]
pub mod database;

// Chains module for blockchain integrations
#[cfg(feature = "database")]
pub mod chains;

// Error handling
#[cfg(feature = "database")]
pub mod error;

// Logging and tracing
#[cfg(feature = "database")]
pub mod logging;

// Middleware
#[cfg(feature = "database")]
pub mod middleware;

// Cache layer
#[cfg(feature = "cache")]
pub mod cache;

// Payment providers
#[cfg(feature = "database")]
pub mod payments;

// Contract error enum for Soroban (only when not using database feature)
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
    /// Initialize the contract with admin settings
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
            // Max 10% (1000 basis points)
            return Err(Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::FeeRate, &fee_rate);
        env.storage()
            .instance()
            .set(&DataKey::FeeTreasury, &fee_treasury);
        env.storage()
            .instance()
            .set(&DataKey::DisputeResolver, &dispute_resolver);
        env.storage().instance().set(&DataKey::IsPaused, &false);
        env.storage().instance().set(&DataKey::OrderCount, &0u64);
        Ok(())
    }

    /// Transfer admin rights to a new address
    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        Ok(())
    }

    /// Update the platform fee rate
    pub fn set_fee_rate(env: Env, new_fee_rate: u32) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        if new_fee_rate > 1000 {
            return Err(Error::InvalidFeeRate);
        }
        env.storage()
            .instance()
            .set(&DataKey::FeeRate, &new_fee_rate);
        Ok(())
    }

    /// Update the fee treasury address
    pub fn set_fee_treasury(env: Env, new_treasury: Address) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::FeeTreasury, &new_treasury);
        Ok(())
    }

    /// Update the dispute resolver address
    pub fn set_dispute_resolver(env: Env, new_resolver: Address) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::DisputeResolver, &new_resolver);
        Ok(())
    }

    /// Pause the contract operations
    pub fn pause(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &true);
        Ok(())
    }

    /// Unpause the contract operations
    pub fn unpause(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &false);
        Ok(())
    }

    /// Check if the contract is paused
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::IsPaused)
            .unwrap_or(false)
    }

    /// Get the current admin address
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)
    }

    /// Accept an open sell order and lock funds in escrow
    pub fn accept_order(env: Env, order_id: u64, buyer: Address) -> Result<(), Error> {
        buyer.require_auth();

        let is_paused: bool = env
            .storage()
            .instance()
            .get(&DataKey::IsPaused)
            .unwrap_or(false);
        if is_paused {
            return Err(Error::ContractPaused);
        }

        let mut order: Order = env
            .storage()
            .persistent()
            .get(&DataKey::Order(order_id))
            .ok_or(Error::OrderNotFound)?;

        Self::validate_order_acceptance(&env, &order, &buyer)?;

        Self::lock_escrow_funds(&env, &order)?;

        order.buyer = Some(buyer.clone());
        order.status = OrderStatus::Locked;

        env.storage()
            .persistent()
            .set(&DataKey::Order(order_id), &order);

        Self::update_user_orders(&env, &buyer, order_id);

        env.events().publish(
            (Symbol::new(&env, "order_accepted"),),
            (order_id, buyer.clone(), order.amount),
        );

        Ok(())
    }

    /// Validate that an order can be accepted by a buyer
    fn validate_order_acceptance(env: &Env, order: &Order, buyer: &Address) -> Result<(), Error> {
        if order.status != OrderStatus::Open {
            return Err(Error::InvalidOrderStatus);
        }

        let current_time = env.ledger().timestamp();
        if current_time > order.expires_at {
            return Err(Error::OrderExpired);
        }

        if buyer == &order.seller {
            return Err(Error::CannotAcceptOwnOrder);
        }

        Ok(())
    }

    /// Lock the seller's crypto funds in the escrow contract
    fn lock_escrow_funds(env: &Env, order: &Order) -> Result<(), Error> {
        let token_client = token::Client::new(env, &order.token);

        token_client.transfer(
            &order.seller,
            &env.current_contract_address(),
            &order.amount,
        );

        Ok(())
    }

    /// Update the user's order list to include the new order
    fn update_user_orders(env: &Env, user: &Address, order_id: u64) {
        let mut user_orders: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::UserOrders(user.clone()))
            .unwrap_or(Vec::new(env));

        user_orders.push_back(order_id);

        env.storage()
            .persistent()
            .set(&DataKey::UserOrders(user.clone()), &user_orders);
    }
}

#[cfg(all(test, not(feature = "database")))]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger};
    use soroban_sdk::{Address, Env};

    fn create_env() -> Env {
        Env::default()
    }

    fn create_addresses(env: &Env) -> (Address, Address, Address, Address) {
        (
            Address::generate(env),
            Address::generate(env),
            Address::generate(env),
            Address::generate(env),
        )
    }

    fn create_token(env: &Env, admin: &Address, user: &Address, amount: i128) -> Address {
        let sac = env.register_stellar_asset_contract_v2(admin.clone());
        sac.address()
    }

    fn create_mock_order(
        env: &Env,
        seller: &Address,
        token: &Address,
        order_id: u64,
        status: OrderStatus,
        expires_at: u64,
    ) -> Order {
        Order {
            id: order_id,
            seller: seller.clone(),
            buyer: None,
            token: token.clone(),
            amount: 1000,
            fiat_currency: Symbol::new(env, "USD"),
            fiat_amount: 100,
            rate: 10,
            status,
            created_at: env.ledger().timestamp(),
            expires_at,
            payment_method: String::from_str(env, "Bank Transfer"),
        }
    }

    #[test]
    fn test_initialize() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        let result = env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
        });
        assert!(result.is_ok());

        let stored_admin = env.as_contract(&contract_id, || {
            EscrowContract::get_admin(env.clone()).unwrap()
        });
        assert_eq!(stored_admin, admin);

        let is_paused = env.as_contract(&contract_id, || EscrowContract::is_paused(env.clone()));
        assert!(!is_paused);
    }

    #[test]
    fn test_prevent_double_initialization() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });
        let result = env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
        });
        assert_eq!(result, Err(Error::AlreadyInitialized));
    }

    #[test]
    fn test_set_fee_rate() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        env.mock_all_auths();
        let result = env.as_contract(&contract_id, || {
            EscrowContract::set_fee_rate(env.clone(), 100)
        });
        assert!(result.is_ok());
    }

    #[test]
    #[should_panic]
    fn test_non_admin_cannot_set_fee_rate() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        env.as_contract(&contract_id, || {
            EscrowContract::set_fee_rate(env.clone(), 100).unwrap();
        });
    }

    #[test]
    fn test_invalid_fee_rate() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        env.mock_all_auths();
        let result = env.as_contract(&contract_id, || {
            EscrowContract::set_fee_rate(env.clone(), 1500)
        });
        assert_eq!(result, Err(Error::InvalidFeeRate));
    }

    #[test]
    fn test_set_admin() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, new_admin) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        env.mock_all_auths();
        env.as_contract(&contract_id, || {
            EscrowContract::set_admin(env.clone(), new_admin.clone()).unwrap();
        });

        let stored_admin = env.as_contract(&contract_id, || {
            EscrowContract::get_admin(env.clone()).unwrap()
        });
        assert_eq!(stored_admin, new_admin);
    }

    #[test]
    fn test_pause_unpause() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        env.mock_all_auths();
        env.as_contract(&contract_id, || {
            EscrowContract::pause(env.clone()).unwrap();
        });
        let paused = env.as_contract(&contract_id, || EscrowContract::is_paused(env.clone()));
        assert!(paused);

        env.as_contract(&contract_id, || {
            EscrowContract::unpause(env.clone()).unwrap();
        });
        let paused = env.as_contract(&contract_id, || EscrowContract::is_paused(env.clone()));
        assert!(!paused);
    }

    #[test]
    fn test_is_paused() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        let paused = env.as_contract(&contract_id, || EscrowContract::is_paused(env.clone()));
        assert!(!paused);
    }

    #[test]
    fn test_get_admin() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        let result = env.as_contract(&contract_id, || EscrowContract::get_admin(env.clone()));
        assert_eq!(result, Ok(admin));
    }

    #[test]
    fn test_accept_order_not_found() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);
        let buyer = Address::generate(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        env.mock_all_auths();
        let result = env.as_contract(&contract_id, || {
            EscrowContract::accept_order(env.clone(), 999, buyer.clone())
        });

        assert_eq!(result, Err(Error::OrderNotFound));
    }

    #[test]
    fn test_accept_order_when_paused() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);
        let buyer = Address::generate(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        env.mock_all_auths();
        env.as_contract(&contract_id, || {
            EscrowContract::pause(env.clone()).unwrap();
        });

        let result = env.as_contract(&contract_id, || {
            EscrowContract::accept_order(env.clone(), 1, buyer.clone())
        });

        assert_eq!(result, Err(Error::ContractPaused));
    }
    #[test]
    fn test_accept_order_invalid_status_locked() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        let seller = Address::generate(&env);
        let buyer = Address::generate(&env);
        let token = Address::generate(&env);
        let order_id = 1u64;

        // Create an order with Locked status
        let order = create_mock_order(
            &env,
            &seller,
            &token,
            order_id,
            OrderStatus::Locked,
            env.ledger().timestamp() + 3600,
        );

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Order(order_id), &order);
        });

        env.mock_all_auths();
        let result = env.as_contract(&contract_id, || {
            EscrowContract::accept_order(env.clone(), order_id, buyer.clone())
        });

        assert_eq!(result, Err(Error::InvalidOrderStatus));
    }

    #[test]
    fn test_accept_order_invalid_status_completed() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        let seller = Address::generate(&env);
        let buyer = Address::generate(&env);
        let token = Address::generate(&env);
        let order_id = 1u64;

        // Create an order with Completed status
        let order = create_mock_order(
            &env,
            &seller,
            &token,
            order_id,
            OrderStatus::Completed,
            env.ledger().timestamp() + 3600,
        );

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Order(order_id), &order);
        });

        env.mock_all_auths();
        let result = env.as_contract(&contract_id, || {
            EscrowContract::accept_order(env.clone(), order_id, buyer.clone())
        });

        assert_eq!(result, Err(Error::InvalidOrderStatus));
    }

    #[test]
    fn test_accept_order_expired() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        let seller = Address::generate(&env);
        let buyer = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token = create_token(&env, &token_admin, &seller, 1000);
        let order_id = 1u64;

        // Set timestamp to avoid overflow
        env.ledger().set_timestamp(1000);

        // Create an order that expires in the past
        let expired_time = env.ledger().timestamp() - 1;
        let order = create_mock_order(
            &env,
            &seller,
            &token,
            order_id,
            OrderStatus::Open,
            expired_time,
        );

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Order(order_id), &order);
        });

        env.mock_all_auths();
        let result = env.as_contract(&contract_id, || {
            EscrowContract::accept_order(env.clone(), order_id, buyer.clone())
        });

        assert_eq!(result, Err(Error::OrderExpired));
    }

    #[test]
    fn test_accept_order_cannot_accept_own() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        let seller = Address::generate(&env);
        let token = Address::generate(&env);
        let order_id = 1u64;

        // Create an open order
        let order = create_mock_order(
            &env,
            &seller,
            &token,
            order_id,
            OrderStatus::Open,
            env.ledger().timestamp() + 3600,
        );

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Order(order_id), &order);
        });

        env.mock_all_auths();
        // Seller tries to accept their own order
        let result = env.as_contract(&contract_id, || {
            EscrowContract::accept_order(env.clone(), order_id, seller.clone())
        });

        assert_eq!(result, Err(Error::CannotAcceptOwnOrder));
    }

    #[test]
    fn test_accept_order_with_disputed_status() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        let seller = Address::generate(&env);
        let buyer = Address::generate(&env);
        let token = Address::generate(&env);
        let order_id = 1u64;

        // Create an order with Disputed status
        let order = create_mock_order(
            &env,
            &seller,
            &token,
            order_id,
            OrderStatus::Disputed,
            env.ledger().timestamp() + 3600,
        );

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Order(order_id), &order);
        });

        env.mock_all_auths();
        let result = env.as_contract(&contract_id, || {
            EscrowContract::accept_order(env.clone(), order_id, buyer.clone())
        });

        assert_eq!(result, Err(Error::InvalidOrderStatus));
    }

    #[test]
    fn test_accept_order_with_cancelled_status() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(
                env.clone(),
                admin.clone(),
                50,
                treasury.clone(),
                resolver.clone(),
            )
            .unwrap();
        });

        let seller = Address::generate(&env);
        let buyer = Address::generate(&env);
        let token = Address::generate(&env);
        let order_id = 1u64;

        // Create an order with Cancelled status
        let order = create_mock_order(
            &env,
            &seller,
            &token,
            order_id,
            OrderStatus::Cancelled,
            env.ledger().timestamp() + 3600,
        );

        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Order(order_id), &order);
        });

        env.mock_all_auths();
        let result = env.as_contract(&contract_id, || {
            EscrowContract::accept_order(env.clone(), order_id, buyer.clone())
        });

        assert_eq!(result, Err(Error::InvalidOrderStatus));
    }
}
