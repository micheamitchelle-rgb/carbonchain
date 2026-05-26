#![no_std]
pub mod types;

use crate::types::{DataKey, RetirementRecord};
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    Address, BytesN, Env, String, Symbol, Vec,
    IntoVal,
};
use soroban_sdk::xdr::ToXdr;

#[contract]
pub struct Retirement;

#[contractimpl]
impl Retirement {
    /// Retire a carbon credit.
    ///
    /// - Stores an immutable `RetirementRecord`
    /// - Calls `mark_retired` on the credit registry to flip the credit status
    /// - Indexes the retirement under the buyer's account
    /// - Emits a `retire` event
    ///
    /// `registry_id` — the deployed credit_registry contract address.
    pub fn retire(
        env: Env,
        buyer: Address,
        credit_id: BytesN<32>,
        tonnes: i128,
        reason: String,
        registry_id: Address,
    ) -> BytesN<32> {
        buyer.require_auth();

        if tonnes <= 0 {
            panic!("tonnes must be greater than zero");
        }

        // Derive a deterministic retirement ID from credit_id + reason
        let mut preimage = credit_id.clone().to_xdr(&env);
        preimage.append(&reason.clone().to_xdr(&env));
        preimage.append(&env.ledger().timestamp().to_xdr(&env));
        let retirement_id: BytesN<32> = env.crypto().sha256(&preimage).into();

        let record = RetirementRecord {
            credit_id: credit_id.clone(),
            buyer: buyer.clone(),
            tonnes_retired: tonnes,
            reason,
            retired_at: env.ledger().timestamp(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::Retirement(retirement_id.clone()), &record);

        // Index under buyer account
        let acct_key = DataKey::AccountRetirements(buyer.clone());
        let mut list: Vec<BytesN<32>> = env
            .storage()
            .persistent()
            .get(&acct_key)
            .unwrap_or_else(|| Vec::new(&env));
        list.push_back(retirement_id.clone());
        env.storage().persistent().set(&acct_key, &list);

        // Cross-contract: mark the credit as retired in the registry
        let _: () = env.invoke_contract(
            &registry_id,
            &Symbol::new(&env, "mark_retired"),
            (credit_id.clone(),).into_val(&env),
        );

        // Emit retirement event
        env.events().publish(
            (symbol_short!("retire"), buyer),
            (credit_id, retirement_id.clone()),
        );

        retirement_id
    }

    pub fn get_retirement(env: Env, retirement_id: BytesN<32>) -> Option<RetirementRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::Retirement(retirement_id))
    }

    pub fn get_retirements_by_account(env: Env, account: Address) -> Vec<BytesN<32>> {
        env.storage()
            .persistent()
            .get(&DataKey::AccountRetirements(account))
            .unwrap_or_else(|| Vec::new(&env))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Env, String};
    use carbonchain_credit_registry::CreditRegistry;

    /// Returns (retirement_contract_id, registry_id, credit_id)
    fn setup(env: &Env) -> (Address, Address, BytesN<32>) {
        // Register retirement first so its address is known for registry init
        let retirement_id = env.register(Retirement, ());
        let registry_id = env.register(CreditRegistry, ());
        let registry_client =
            carbonchain_credit_registry::CreditRegistryClient::new(env, &registry_id);

        let admin = Address::generate(env);
        let verifier = Address::generate(env);
        let issuer = Address::generate(env);

        registry_client.initialize(&admin, &retirement_id);
        registry_client.register_verifier(&admin, &verifier);

        let credit_id = registry_client.submit_credit(
            &issuer,
            &String::from_str(env, "PROJ-001"),
            &2024,
            &String::from_str(env, "VCS"),
            &String::from_str(env, "NG"),
            &1_000_000,
            &String::from_str(env, "bafybei123"),
        );
        registry_client.approve_and_mint(&verifier, &credit_id);

        (retirement_id, registry_id, credit_id)
    }

    #[test]
    fn test_duplicate_retirement_same_reason_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (registry_id, _admin, _verifier, credit_id) = setup_registry(&env);

        let contract_id = env.register(Retirement, ());
        let client = RetirementClient::new(&env, &contract_id);
        let buyer = Address::generate(&env);
        let reason = String::from_str(&env, "2024 Scope 3 offset");

        // First retirement succeeds.
        client.retire(&buyer, &credit_id, &1_000_000, &reason, &registry_id);

        // Second retirement with the same credit must fail because the registry
        // rejects mark_retired on an already-retired credit.
        let result = client.try_retire(&buyer, &credit_id, &1_000_000, &reason, &registry_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_retire_stores_record() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, registry_id, credit_id) = setup(&env);
        let client = RetirementClient::new(&env, &contract_id);
        let buyer = Address::generate(&env);

        let ret_id = client.retire(
            &buyer,
            &credit_id,
            &1_000_000,
            &String::from_str(&env, "2024 Scope 3 offset"),
            &registry_id,
        );

        let record = client.get_retirement(&ret_id).unwrap();
        assert_eq!(record.buyer, buyer);
        assert_eq!(record.tonnes_retired, 1_000_000);
        assert_eq!(record.credit_id, credit_id);
    }

    #[test]
    fn test_retire_indexes_by_account() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, registry_id, credit_id) = setup(&env);
        let client = RetirementClient::new(&env, &contract_id);
        let buyer = Address::generate(&env);

        let ret_id = client.retire(
            &buyer,
            &credit_id,
            &1_000_000,
            &String::from_str(&env, "offset"),
            &registry_id,
        );

        let ids = client.get_retirements_by_account(&buyer);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.get(0).unwrap(), ret_id);
    }

    #[test]
    fn test_retire_marks_credit_retired_in_registry() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, registry_id, credit_id) = setup(&env);
        let registry_client =
            carbonchain_credit_registry::CreditRegistryClient::new(&env, &registry_id);
        let client = RetirementClient::new(&env, &contract_id);
        let buyer = Address::generate(&env);

        client.retire(
            &buyer,
            &credit_id,
            &1_000_000,
            &String::from_str(&env, "offset"),
            &registry_id,
        );

        let credit = registry_client.get_credit(&credit_id);
        assert_eq!(
            credit.status,
            carbonchain_credit_registry::types::CreditStatus::Retired
        );
    }

    #[test]
    #[should_panic]
    fn test_retire_zero_tonnes_fails() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, registry_id, credit_id) = setup(&env);
        let client = RetirementClient::new(&env, &contract_id);
        let buyer = Address::generate(&env);

        client.retire(
            &buyer,
            &credit_id,
            &0,
            &String::from_str(&env, "offset"),
            &registry_id,
        );
    }
}
