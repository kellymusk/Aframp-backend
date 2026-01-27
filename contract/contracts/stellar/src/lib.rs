#![no_std]

mod contract;
mod storage;
mod events;
mod error;

use soroban_sdk::{contract, contractimpl, Env, Address};

#[contract]
pub struct AfrIContract;

#[contractimpl]
impl AfrIContract {
    pub fn init(env: Env, admin: Address) {
        storage::set_admin(&env, &admin);
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        contract::mint(env, to, amount);
    }

    pub fn burn(env: Env, from: Address, amount: i128) {
        contract::burn(env, from, amount);
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        contract::transfer(env, from, to, amount);
    }

    pub fn balance(env: Env, user: Address) -> i128 {
        storage::get_balance(&env, &user)
    }
}
