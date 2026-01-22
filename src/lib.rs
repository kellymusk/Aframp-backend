#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, Address, Env, Symbol, String};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidFeeRate = 4,
    ContractPaused = 5,
}

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

#[contract]
pub struct EscrowContract;

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
        if fee_rate > 1000 { // Max 10% (1000 basis points)
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

    /// Transfer admin rights to a new address
    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        Ok(())
    }

    /// Update the platform fee rate
    pub fn set_fee_rate(env: Env, new_fee_rate: u32) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        if new_fee_rate > 1000 {
            return Err(Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::FeeRate, &new_fee_rate);
        Ok(())
    }

    /// Update the fee treasury address
    pub fn set_fee_treasury(env: Env, new_treasury: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::FeeTreasury, &new_treasury);
        Ok(())
    }

    /// Update the dispute resolver address
    pub fn set_dispute_resolver(env: Env, new_resolver: Address) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::DisputeResolver, &new_resolver);
        Ok(())
    }

    /// Pause the contract operations
    pub fn pause(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &true);
        Ok(())
    }

    /// Unpause the contract operations
    pub fn unpause(env: Env) -> Result<(), Error> {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::IsPaused, &false);
        Ok(())
    }

    /// Check if the contract is paused
    pub fn is_paused(env: Env) -> bool {
        env.storage().instance().get(&DataKey::IsPaused).unwrap_or(false)
    }

    /// Get the current admin address
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage().instance().get(&DataKey::Admin).ok_or(Error::NotInitialized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
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

    #[test]
    fn test_initialize() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        let result = env.as_contract(&contract_id, || {
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone())
        });
        assert!(result.is_ok());

        let stored_admin = env.as_contract(&contract_id, || {
            EscrowContract::get_admin(env.clone()).unwrap()
        });
        assert_eq!(stored_admin, admin);

        let is_paused = env.as_contract(&contract_id, || {
            EscrowContract::is_paused(env.clone())
        });
        assert!(!is_paused);
    }

    #[test]
    fn test_prevent_double_initialization() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
        });
        let result = env.as_contract(&contract_id, || {
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone())
        });
        assert_eq!(result, Err(Error::AlreadyInitialized));
    }

    #[test]
    fn test_set_fee_rate() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
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
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
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
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
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
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
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
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
        });

        env.mock_all_auths();
        env.as_contract(&contract_id, || {
            EscrowContract::pause(env.clone()).unwrap();
        });
        let paused = env.as_contract(&contract_id, || {
            EscrowContract::is_paused(env.clone())
        });
        assert!(paused);

        env.as_contract(&contract_id, || {
            EscrowContract::unpause(env.clone()).unwrap();
        });
        let paused = env.as_contract(&contract_id, || {
            EscrowContract::is_paused(env.clone())
        });
        assert!(!paused);
    }

    #[test]
    fn test_is_paused() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
        });

        let paused = env.as_contract(&contract_id, || {
            EscrowContract::is_paused(env.clone())
        });
        assert!(!paused);
    }

    #[test]
    fn test_get_admin() {
        let env = create_env();
        let contract_id = env.register_contract(None, EscrowContract);
        let (admin, treasury, resolver, _) = create_addresses(&env);

        env.as_contract(&contract_id, || {
            EscrowContract::initialize(env.clone(), admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
        });

        let result = env.as_contract(&contract_id, || {
            EscrowContract::get_admin(env.clone())
        });
        assert_eq!(result, Ok(admin));
    }
}