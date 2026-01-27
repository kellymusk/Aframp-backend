#![allow(deprecated)]

use soroban_sdk::{Env, Address, symbol_short};

pub fn transfer(env: &Env, from: &Address, to: &Address, amount: i128) {
    env.events().publish(
        (
            symbol_short!("transfer"),
            from.clone(),
            to.clone(),
        ),
        amount,
    );
}

pub fn mint(env: &Env, to: &Address, amount: i128) {
    env.events().publish(
        (
            symbol_short!("mint"),
            to.clone(),
        ),
        amount,
    );
}

pub fn burn(env: &Env, from: &Address, amount: i128) {
    env.events().publish(
        (
            symbol_short!("burn"),
            from.clone(),
        ),
        amount,
    );
}
