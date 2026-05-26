use soroban_sdk::{Env, Address, BytesN, Vec, String};
use crate::types::{DataKey, CreditMetadata};

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

pub fn get_admin(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Admin)
}

pub fn has_admin(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Admin)
}

pub fn set_credit(env: &Env, id: &BytesN<32>, metadata: &CreditMetadata) {
    env.storage().persistent().set(&DataKey::Credit(id.clone()), metadata);
}

pub fn get_credit(env: &Env, id: &BytesN<32>) -> Option<CreditMetadata> {
    env.storage().persistent().get(&DataKey::Credit(id.clone()))
}

pub fn get_verifiers(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::VerifierSet)
        .unwrap_or_else(|| Vec::new(env))
}

pub fn set_verifiers(env: &Env, verifiers: &Vec<Address>) {
    env.storage().instance().set(&DataKey::VerifierSet, verifiers);
}

pub fn is_verifier(env: &Env, verifier: &Address) -> bool {
    get_verifiers(env).contains(verifier)
}

/// Append a credit id to the per-project index.
pub fn add_credit_to_project(env: &Env, project_id: &String, credit_id: &BytesN<32>) {
    let key = DataKey::ProjectCredits(project_id.clone());
    let mut list: Vec<BytesN<32>> = env.storage().persistent().get(&key).unwrap_or_else(|| Vec::new(env));
    list.push_back(credit_id.clone());
    env.storage().persistent().set(&key, &list);
}

pub fn get_credits_by_project(env: &Env, project_id: &String) -> Vec<BytesN<32>> {
    env.storage()
        .persistent()
        .get(&DataKey::ProjectCredits(project_id.clone()))
        .unwrap_or_else(|| Vec::new(env))
}

pub fn set_retirement_contract(env: &Env, addr: &Address) {
    env.storage().instance().set(&DataKey::RetirementContract, addr);
}

pub fn get_retirement_contract(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::RetirementContract)
}
