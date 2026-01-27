#![no_std]

mod storage;

use soroban_sdk::{contract, contractimpl, Address, Env};

#[contract]
pub struct AfrIContract;

#[contractimpl]
impl AfrIContract {
    pub fn init(env: Env, admin: Address) {
        storage::set_admin(&env, &admin);
    }

    // Mint now requires the caller address for admin check
    pub fn mint(env: Env, caller: Address, to: Address, amount: i128) {
        let admin = storage::get_admin(&env);
        if caller != admin {
            panic!("Only admin can mint");
        }
        let current = storage::get_balance(&env, &to);
        storage::set_balance(&env, &to, current + amount);
    }

    pub fn burn(env: Env, from: Address, amount: i128) {
        let current = storage::get_balance(&env, &from);
        if amount > current {
            panic!("Insufficient balance to burn");
        }
        storage::set_balance(&env, &from, current - amount);
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let from_balance = storage::get_balance(&env, &from);
        if amount > from_balance {
            panic!("Insufficient balance to transfer");
        }
        storage::set_balance(&env, &from, from_balance - amount);
        storage::set_balance(&env, &to, storage::get_balance(&env, &to) + amount);
    }

    pub fn balance(env: Env, user: Address) -> i128 {
        storage::get_balance(&env, &user)
    }
}

mod test;
