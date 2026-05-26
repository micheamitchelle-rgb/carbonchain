use soroban_sdk::{Env, Address, BytesN, symbol_short, String};

pub fn credit_submitted(env: &Env, issuer: Address, project_id: String, tonnes: i128) {
    let topics = (symbol_short!("submit"), issuer);
    env.events().publish(topics, (project_id, tonnes));
}

pub fn credit_minted(env: &Env, verifier: Address, id: BytesN<32>) {
    let topics = (symbol_short!("mint"), verifier);
    env.events().publish(topics, id);
}

pub fn credit_flagged(env: &Env, id: BytesN<32>, reason: String) {
    let topics = (symbol_short!("flag"),);
    env.events().publish(topics, (id, reason));
}

pub fn verifier_added(env: &Env, admin: Address, verifier: Address) {
    let topics = (symbol_short!("ver_add"), admin);
    env.events().publish(topics, verifier);
}

pub fn verifier_removed(env: &Env, admin: Address, verifier: Address) {
    let topics = (symbol_short!("ver_rm"), admin);
    env.events().publish(topics, verifier);
}

pub fn contract_paused(env: &Env, admin: Address) {
    env.events().publish((symbol_short!("paused"),), admin);
}

pub fn contract_unpaused(env: &Env, admin: Address) {
    env.events().publish((symbol_short!("unpaused"),), admin);
}
