#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Env as _};
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
        let (admin, treasury, resolver, _) = create_addresses(&env);

        let result = EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone());
        assert!(result.is_ok());

        let stored_admin = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert_eq!(stored_admin, admin);
        assert!(!EscrowContract::is_paused(&env));
    }

    #[test]
    fn test_prevent_double_initialization() {
        let env = create_env();
        let (admin, treasury, resolver, _) = create_addresses(&env);

        EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();
        let result = EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone());
        assert_eq!(result, Err(Error::AlreadyInitialized));
    }

    #[test]
    fn test_set_fee_rate() {
        let env = create_env();
        let (admin, treasury, resolver, _) = create_addresses(&env);

        EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();

        env.mock_auths(&[admin.clone()]);
        let result = EscrowContract::set_fee_rate(&env, 100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_non_admin_cannot_set_fee_rate() {
        let env = create_env();
        let (admin, treasury, resolver, non_admin) = create_addresses(&env);

        EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();

        env.mock_auths(&[non_admin.clone()]);
        let result = EscrowContract::set_fee_rate(&env, 100);
        assert_eq!(result, Err(Error::Unauthorized));
    }

    #[test]
    fn test_invalid_fee_rate() {
        let env = create_env();
        let (admin, treasury, resolver, _) = create_addresses(&env);

        EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();

        env.mock_auths(&[admin.clone()]);
        let result = EscrowContract::set_fee_rate(&env, 1500);
        assert_eq!(result, Err(Error::InvalidFeeRate));
    }

    #[test]
    fn test_set_admin() {
        let env = create_env();
        let (admin, treasury, resolver, new_admin) = create_addresses(&env);

        EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();

        env.mock_auths(&[admin.clone()]);
        EscrowContract::set_admin(&env, new_admin.clone()).unwrap();

        let stored_admin = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert_eq!(stored_admin, new_admin);
    }

    #[test]
    fn test_pause_unpause() {
        let env = create_env();
        let (admin, treasury, resolver, _) = create_addresses(&env);

        EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();

        env.mock_auths(&[admin.clone()]);
        EscrowContract::pause(&env).unwrap();
        assert!(EscrowContract::is_paused(&env));

        env.mock_auths(&[admin.clone()]);
        EscrowContract::unpause(&env).unwrap();
        assert!(!EscrowContract::is_paused(&env));
    }

    #[test]
    fn test_is_paused() {
        let env = create_env();
        let (admin, treasury, resolver, _) = create_addresses(&env);

        EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();

        assert!(!EscrowContract::is_paused(&env));
    }

    #[test]
    fn test_get_admin() {
        let env = create_env();
        let (admin, treasury, resolver, _) = create_addresses(&env);

        EscrowContract::initialize(&env, admin.clone(), 50, treasury.clone(), resolver.clone()).unwrap();

        let result = EscrowContract::get_admin(&env);
        assert_eq!(result, Ok(admin));
    }
}