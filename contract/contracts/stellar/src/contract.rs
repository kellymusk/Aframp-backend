use soroban_sdk::{Env, Address, panic_with_error};

use crate::{storage, events};
use crate::error::ContractError;

pub fn mint(env: Env, to: Address, amount: i128) {
    if amount <= 0 {
        panic_with_error!(&env, ContractError::InvalidAmount);
    }

    let admin = storage::get_admin(&env);
    admin.require_auth();

    let bal = storage::get_balance(&env, &to);
    storage::set_balance(&env, &to, bal + amount);

    events::mint(&env, &to, amount);
}

pub fn burn(env: Env, from: Address, amount: i128) {
    if amount <= 0 {
        panic_with_error!(&env, ContractError::InvalidAmount);
    }

    let admin = storage::get_admin(&env);
    admin.require_auth();

    let bal = storage::get_balance(&env, &from);
    if bal < amount {
        panic_with_error!(&env, ContractError::InsufficientBalance);
    }

    storage::set_balance(&env, &from, bal - amount);
    events::burn(&env, &from, amount);
}

pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
    if amount <= 0 {
        panic_with_error!(&env, ContractError::InvalidAmount);
    }

    from.require_auth();

    let from_bal = storage::get_balance(&env, &from);
    if from_bal < amount {
        panic_with_error!(&env, ContractError::InsufficientBalance);
    }

    let to_bal = storage::get_balance(&env, &to);

    storage::set_balance(&env, &from, from_bal - amount);
    storage::set_balance(&env, &to, to_bal + amount);

    events::transfer(&env, &from, &to, amount);
}
