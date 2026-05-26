#![no_std]
use soroban_sdk::{contract, contractimpl, Env, Address, String, BytesN, Vec};
use soroban_sdk::xdr::ToXdr;

pub mod types;
pub mod errors;
pub mod storage;
pub mod events;

use crate::errors::CarbonChainError;
use crate::storage::{
    set_admin, get_admin, has_admin,
    set_credit, get_credit,
    get_verifiers, set_verifiers, is_verifier,
    add_credit_to_project, get_credits_by_project,
    set_retirement_contract, get_retirement_contract,
};
use crate::types::{CreditMetadata, CreditStatus};
use crate::events::{credit_submitted, credit_minted, verifier_added, verifier_removed};

#[contract]
pub struct CreditRegistry;

#[contractimpl]
impl CreditRegistry {
    // ── Admin ────────────────────────────────────────────────────────────────

    pub fn initialize(env: Env, admin: Address, retirement_contract: Address) -> Result<(), CarbonChainError> {
        if has_admin(&env) {
            return Err(CarbonChainError::AlreadyInitialized);
        }
        set_admin(&env, &admin);
        set_retirement_contract(&env, &retirement_contract);
        Ok(())
    }

    // ── Verifier management ──────────────────────────────────────────────────

    pub fn register_verifier(env: Env, admin: Address, verifier: Address) -> Result<(), CarbonChainError> {
        let stored_admin = get_admin(&env).ok_or(CarbonChainError::NotInitialized)?;
        admin.require_auth();
        if admin != stored_admin {
            return Err(CarbonChainError::Unauthorized);
        }
        if is_verifier(&env, &verifier) {
            return Err(CarbonChainError::VerifierAlreadyExists);
        }
        let mut verifiers = get_verifiers(&env);
        verifiers.push_back(verifier.clone());
        set_verifiers(&env, &verifiers);
        verifier_added(&env, admin, verifier);
        Ok(())
    }

    pub fn remove_verifier(env: Env, admin: Address, verifier: Address) -> Result<(), CarbonChainError> {
        let stored_admin = get_admin(&env).ok_or(CarbonChainError::NotInitialized)?;
        admin.require_auth();
        if admin != stored_admin {
            return Err(CarbonChainError::Unauthorized);
        }
        if !is_verifier(&env, &verifier) {
            return Err(CarbonChainError::VerifierNotFound);
        }
        let old = get_verifiers(&env);
        let mut new_list: Vec<Address> = Vec::new(&env);
        for v in old.iter() {
            if v != verifier {
                new_list.push_back(v);
            }
        }
        set_verifiers(&env, &new_list);
        verifier_removed(&env, admin, verifier);
        Ok(())
    }

    pub fn list_verifiers(env: Env) -> Vec<Address> {
        get_verifiers(&env)
    }

    // ── Credit lifecycle ─────────────────────────────────────────────────────

    pub fn submit_credit(
        env: Env,
        issuer: Address,
        project_id: String,
        vintage_year: u32,
        methodology: String,
        geography: String,
        tonnes: i128,
        ipfs_hash: String,
    ) -> Result<BytesN<32>, CarbonChainError> {
        if !has_admin(&env) {
            return Err(CarbonChainError::NotInitialized);
        }
        issuer.require_auth();
        if tonnes <= 0 {
            return Err(CarbonChainError::InvalidTonnes);
        }
        // 1 billion tonnes upper bound (in kg units: 1e15)
        if tonnes > 1_000_000_000_000_000 {
            return Err(CarbonChainError::InvalidTonnes);
        }

        // Include a per-contract nonce so two credits for the same project get distinct IDs.
        let nonce: u64 = env.storage().instance().get(&crate::types::DataKey::CreditNonce).unwrap_or(0u64);
        env.storage().instance().set(&crate::types::DataKey::CreditNonce, &(nonce + 1));
        let mut preimage = project_id.clone().to_xdr(&env);
        preimage.append(&nonce.to_xdr(&env));
        let id: BytesN<32> = env.crypto().sha256(&preimage).into();
        let metadata = CreditMetadata {
            project_id: project_id.clone(),
            issuer: issuer.clone(),
            vintage_year,
            methodology,
            geography,
            tonnes,
            ipfs_hash,
            status: CreditStatus::Pending,
            issued_at: env.ledger().timestamp(),
        };

        set_credit(&env, &id, &metadata);
        add_credit_to_project(&env, &project_id, &id);
        credit_submitted(&env, issuer, project_id, tonnes);

        Ok(id)
    }

    pub fn approve_and_mint(env: Env, verifier: Address, credit_id: BytesN<32>) -> Result<(), CarbonChainError> {
        verifier.require_auth();
        if !is_verifier(&env, &verifier) {
            return Err(CarbonChainError::Unauthorized);
        }
        let mut credit = get_credit(&env, &credit_id).ok_or(CarbonChainError::CreditNotFound)?;
        if credit.status != CreditStatus::Pending {
            return Err(CarbonChainError::InvalidStatusTransition);
        }
        credit.status = CreditStatus::Active;
        set_credit(&env, &credit_id, &credit);
        credit_minted(&env, verifier, credit_id);
        Ok(())
    }

    pub fn flag_credit(env: Env, verifier: Address, credit_id: BytesN<32>, reason: String) -> Result<(), CarbonChainError> {
        verifier.require_auth();
        if !is_verifier(&env, &verifier) {
            return Err(CarbonChainError::Unauthorized);
        }
        let mut credit = get_credit(&env, &credit_id).ok_or(CarbonChainError::CreditNotFound)?;
        if credit.status == CreditStatus::Retired {
            return Err(CarbonChainError::InvalidStatusTransition);
        }
        credit.status = CreditStatus::Flagged;
        set_credit(&env, &credit_id, &credit);
        crate::events::credit_flagged(&env, credit_id, reason);
        Ok(())
    }

    pub fn mark_retired(env: Env, credit_id: BytesN<32>) -> Result<(), CarbonChainError> {
        // Only the registered retirement contract may call this.
        let retirement_contract = get_retirement_contract(&env)
            .ok_or(CarbonChainError::NotInitialized)?;
        retirement_contract.require_auth();
        let mut credit = get_credit(&env, &credit_id).ok_or(CarbonChainError::CreditNotFound)?;
        if credit.status != CreditStatus::Active {
            return Err(CarbonChainError::InvalidStatusTransition);
        }
        credit.status = CreditStatus::Retired;
        set_credit(&env, &credit_id, &credit);
        Ok(())
    }

    // ── Queries ──────────────────────────────────────────────────────────────

    pub fn get_credit(env: Env, credit_id: BytesN<32>) -> Result<CreditMetadata, CarbonChainError> {
        get_credit(&env, &credit_id).ok_or(CarbonChainError::CreditNotFound)
    }

    pub fn list_credits_by_project(env: Env, project_id: String) -> Vec<BytesN<32>> {
        get_credits_by_project(&env, &project_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Env, String};

    fn setup() -> (Env, CreditRegistryClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(CreditRegistry, ());
        let client = CreditRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let verifier = Address::generate(&env);
        let retirement = Address::generate(&env);
        client.initialize(&admin, &retirement);
        (env, client, admin, verifier)
    }

    fn submit_test_credit(env: &Env, client: &CreditRegistryClient, issuer: &Address) -> BytesN<32> {
        client.submit_credit(
            issuer,
            &String::from_str(env, "PROJ-001"),
            &2024,
            &String::from_str(env, "VCS"),
            &String::from_str(env, "NG"),
            &1_000_000,
            &String::from_str(env, "bafybei123"),
        )
    }

    #[test]
    fn test_initialize_twice_fails() {
        let (env, client, admin, _) = setup();
        let retirement = Address::generate(&env);
        let result = client.try_initialize(&admin, &retirement);
        assert!(result.is_err());
    }

    #[test]
    fn test_register_and_list_verifier() {
        let (_env, client, admin, verifier) = setup();
        client.register_verifier(&admin, &verifier);
        let list = client.list_verifiers();
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap(), verifier);
    }

    #[test]
    fn test_register_verifier_twice_fails() {
        let (_env, client, admin, verifier) = setup();
        client.register_verifier(&admin, &verifier);
        let result = client.try_register_verifier(&admin, &verifier);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_verifier() {
        let (_env, client, admin, verifier) = setup();
        client.register_verifier(&admin, &verifier);
        client.remove_verifier(&admin, &verifier);
        assert_eq!(client.list_verifiers().len(), 0);
    }

    #[test]
    fn test_submit_credit() {
        let (env, client, _, _) = setup();
        let issuer = Address::generate(&env);
        let id = submit_test_credit(&env, &client, &issuer);
        let credit = client.get_credit(&id);
        assert_eq!(credit.status, CreditStatus::Pending);
        assert_eq!(credit.tonnes, 1_000_000);
    }

    #[test]
    fn test_approve_and_mint() {
        let (env, client, admin, verifier) = setup();
        client.register_verifier(&admin, &verifier);
        let issuer = Address::generate(&env);
        let id = submit_test_credit(&env, &client, &issuer);
        client.approve_and_mint(&verifier, &id);
        assert_eq!(client.get_credit(&id).status, CreditStatus::Active);
    }

    #[test]
    fn test_approve_non_pending_fails() {
        let (env, client, admin, verifier) = setup();
        client.register_verifier(&admin, &verifier);
        let issuer = Address::generate(&env);
        let id = submit_test_credit(&env, &client, &issuer);
        client.approve_and_mint(&verifier, &id);
        // second approval should fail
        let result = client.try_approve_and_mint(&verifier, &id);
        assert!(result.is_err());
    }

    #[test]
    fn test_flag_credit() {
        let (env, client, admin, verifier) = setup();
        client.register_verifier(&admin, &verifier);
        let issuer = Address::generate(&env);
        let id = submit_test_credit(&env, &client, &issuer);
        client.flag_credit(&verifier, &id, &String::from_str(&env, "suspicious data"));
        assert_eq!(client.get_credit(&id).status, CreditStatus::Flagged);
    }

    #[test]
    fn test_mark_retired() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(CreditRegistry, ());
        let client = CreditRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let verifier = Address::generate(&env);
        // Use a generated address as the retirement contract so mock_all_auths covers it
        let retirement = Address::generate(&env);
        client.initialize(&admin, &retirement);
        client.register_verifier(&admin, &verifier);
        let issuer = Address::generate(&env);
        let id = submit_test_credit(&env, &client, &issuer);
        client.approve_and_mint(&verifier, &id);
        client.mark_retired(&id);
        assert_eq!(client.get_credit(&id).status, CreditStatus::Retired);
    }

    #[test]
    fn test_unauthorized_mark_retired_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(CreditRegistry, ());
        let client = CreditRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let verifier = Address::generate(&env);
        let retirement = Address::generate(&env);

        client.initialize(&admin, &retirement);
        client.register_verifier(&admin, &verifier);
        let issuer = Address::generate(&env);
        let id = submit_test_credit(&env, &client, &issuer);
        client.approve_and_mint(&verifier, &id);

        // Disable auth mocking — require_auth calls will now actually enforce auth
        env.set_auths(&[]);

        // mark_retired requires the retirement contract's auth, which is not provided
        let result = client.try_mark_retired(&id);
        assert!(result.is_err());
    }

    #[test]
    fn test_submit_credit_zero_tonnes_fails() {
        let (env, client, _, _) = setup();
        let issuer = Address::generate(&env);
        let result = client.try_submit_credit(
            &issuer,
            &String::from_str(&env, "PROJ-001"),
            &2024,
            &String::from_str(&env, "VCS"),
            &String::from_str(&env, "NG"),
            &0,
            &String::from_str(&env, "bafybei123"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_submit_credit_negative_tonnes_fails() {
        let (env, client, _, _) = setup();
        let issuer = Address::generate(&env);
        let result = client.try_submit_credit(
            &issuer,
            &String::from_str(&env, "PROJ-001"),
            &2024,
            &String::from_str(&env, "VCS"),
            &String::from_str(&env, "NG"),
            &-1,
            &String::from_str(&env, "bafybei123"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_submit_credit_over_upper_bound_fails() {
        let (env, client, _, _) = setup();
        let issuer = Address::generate(&env);
        let result = client.try_submit_credit(
            &issuer,
            &String::from_str(&env, "PROJ-001"),
            &2024,
            &String::from_str(&env, "VCS"),
            &String::from_str(&env, "NG"),
            &1_000_000_000_000_001,
            &String::from_str(&env, "bafybei123"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_submit_credit_at_upper_bound_succeeds() {
        let (env, client, _, _) = setup();
        let issuer = Address::generate(&env);
        let result = client.try_submit_credit(
            &issuer,
            &String::from_str(&env, "PROJ-001"),
            &2024,
            &String::from_str(&env, "VCS"),
            &String::from_str(&env, "NG"),
            &1_000_000_000_000_000,
            &String::from_str(&env, "bafybei123"),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_credits_by_project() {
        let (env, client, _, _) = setup();
        let issuer = Address::generate(&env);
        submit_test_credit(&env, &client, &issuer);
        let ids = client.list_credits_by_project(&String::from_str(&env, "PROJ-001"));
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn test_non_verifier_cannot_approve() {
        let (env, client, _, _) = setup();
        let issuer = Address::generate(&env);
        let id = submit_test_credit(&env, &client, &issuer);
        let fake = Address::generate(&env);
        let result = client.try_approve_and_mint(&fake, &id);
        assert!(result.is_err());
    }
}
