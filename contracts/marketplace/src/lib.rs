#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Env, Address, BytesN, Symbol, Vec, IntoVal};

// ── TTL constants ─────────────────────────────────────────────────────────────
/// Minimum TTL in ledgers (~1 year at 5s/ledger).
const MIN_TTL: u32 = 6_307_200;
/// Threshold below which TTL is extended.
const TTL_THRESHOLD: u32 = MIN_TTL / 2;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct Offer {
    pub seller: Address,
    pub credit_id: BytesN<32>,
    pub price_xlm: i128,   // in stroops
    pub tonnes: i128,
    pub active: bool,
    pub created_at: u64,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Offer(u64),
    OfferCount,
    SellerOffers(Address),
    Admin,
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MarketplaceError {
    OfferNotFound   = 115,
    Unauthorized    = 116,
    InvalidPrice    = 117,
    AlreadyClosed   = 118,
    CreditNotActive = 119,
    NotInitialized  = 120,
    ContractPaused  = 121,
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct Marketplace;

#[contractimpl]
impl Marketplace {
    // ── Admin / Pause ────────────────────────────────────────────────────────

    pub fn initialize(env: Env, admin: Address) -> Result<(), MarketplaceError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(MarketplaceError::NotInitialized); // already initialised
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        Ok(())
    }

    pub fn pause(env: Env, admin: Address) -> Result<(), MarketplaceError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((symbol_short!("paused"),), admin);
        Ok(())
    }

    pub fn unpause(env: Env, admin: Address) -> Result<(), MarketplaceError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((symbol_short!("unpaused"),), admin);
        Ok(())
    }

    pub fn paused(env: Env) -> bool {
        env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
    }

    // ── Offers ───────────────────────────────────────────────────────────────

    /// List a credit for sale. Returns the new offer ID.
    pub fn create_offer(
        env: Env,
        seller: Address,
        credit_id: BytesN<32>,
        price_xlm: i128,
        tonnes: i128,
        registry_id: Address,
        nonce: u64,
    ) -> Result<u64, MarketplaceError> {
        if Self::is_paused(&env) {
            return Err(MarketplaceError::ContractPaused);
        }
        seller.require_auth();
        if !Self::consume_nonce(&env, &seller, nonce) {
            return Err(MarketplaceError::InvalidNonce);
        }
        if price_xlm <= 0 || tonnes <= 0 {
            return Err(MarketplaceError::InvalidPrice);
        }

        // Validate credit exists and is Active in the registry
        let credit: carbonchain_credit_registry::types::CreditMetadata = env.invoke_contract(
            &registry_id,
            &Symbol::new(&env, "get_credit"),
            (credit_id.clone(),).into_val(&env),
        );
        if credit.status != carbonchain_credit_registry::types::CreditStatus::Active {
            return Err(MarketplaceError::CreditNotActive);
        }

        let offer_id = Self::next_id(&env);
        let offer = Offer {
            seller: seller.clone(),
            credit_id,
            price_xlm,
            tonnes,
            active: true,
            created_at: env.ledger().timestamp(),
        };

        env.storage().persistent().set(&DataKey::Offer(offer_id), &offer);
        env.storage().persistent().extend_ttl(&DataKey::Offer(offer_id), TTL_THRESHOLD, MIN_TTL);

        // Index under seller
        let key = DataKey::SellerOffers(seller.clone());
        let mut ids: Vec<u64> = env.storage().persistent().get(&key).unwrap_or_else(|| Vec::new(&env));
        ids.push_back(offer_id);
        env.storage().persistent().set(&key, &ids);
        env.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, MIN_TTL);

        env.events().publish((symbol_short!("offer_new"), seller), offer_id);
        Ok(offer_id)
    }

    /// Cancel an open offer. Only the original seller may cancel.
    ///
    /// Emits an `offer_cxl` event **only** on success. Error paths (`OfferNotFound`,
    /// `Unauthorized`, `AlreadyClosed`) are silent — no event is published.
    pub fn cancel_offer(env: Env, seller: Address, offer_id: u64) -> Result<(), MarketplaceError> {
        if Self::is_paused(&env) {
            return Err(MarketplaceError::ContractPaused);
        }
        seller.require_auth();
        if !Self::consume_nonce(&env, &seller, nonce) {
            return Err(MarketplaceError::InvalidNonce);
        }
        let mut offer: Offer = env
            .storage()
            .persistent()
            .get(&DataKey::Offer(offer_id))
            .ok_or(MarketplaceError::OfferNotFound)?;

        if offer.seller != seller {
            return Err(MarketplaceError::Unauthorized);
        }
        if !offer.active {
            return Err(MarketplaceError::AlreadyClosed);
        }

        offer.active = false;
        env.storage().persistent().set(&DataKey::Offer(offer_id), &offer);
        env.storage().persistent().extend_ttl(&DataKey::Offer(offer_id), TTL_THRESHOLD, MIN_TTL);
        env.events().publish((symbol_short!("offer_cxl"), seller), offer_id);
        Ok(())
    }

    pub fn get_offer(env: Env, offer_id: u64) -> Result<Offer, MarketplaceError> {
        env.storage()
            .persistent()
            .get(&DataKey::Offer(offer_id))
            .ok_or(MarketplaceError::OfferNotFound)
    }

    /// Returns all offer IDs for a seller (including cancelled ones).
    pub fn get_offers_by_seller(env: Env, seller: Address) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::SellerOffers(seller))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Returns only the active (non-cancelled) offer IDs for a seller.
    /// Avoids callers having to fetch each offer individually to filter.
    pub fn get_active_offers_by_seller(env: Env, seller: Address) -> Vec<u64> {
        let all_ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::SellerOffers(seller))
            .unwrap_or_else(|| Vec::new(&env));

        let mut active: Vec<u64> = Vec::new(&env);
        for id in all_ids.iter() {
            let offer: Option<Offer> = env.storage().persistent().get(&DataKey::Offer(id));
            if let Some(o) = offer {
                if o.active {
                    active.push_back(id);
                }
            }
        }
        active
    }

    pub fn offer_count(env: Env) -> u64 {
        env.storage().persistent().get(&DataKey::OfferCount).unwrap_or(0u64)
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    fn next_id(env: &Env) -> u64 {
        let id: u64 = env.storage().persistent().get(&DataKey::OfferCount).unwrap_or(0u64);
        env.storage().persistent().set(&DataKey::OfferCount, &(id + 1));
        env.storage().persistent().extend_ttl(&DataKey::OfferCount, TTL_THRESHOLD, MIN_TTL);
        id
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), MarketplaceError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(MarketplaceError::NotInitialized)?;
        caller.require_auth();
        if *caller != admin {
            return Err(MarketplaceError::Unauthorized);
        }
        Ok(())
    }

    fn is_paused(env: &Env) -> bool {
        env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Env, BytesN, String};
    use carbonchain_credit_registry::CreditRegistry;

    fn setup_with_registry(env: &Env) -> (MarketplaceClient<'static>, Address, Address, Address, BytesN<32>) {
        let registry_id = env.register(CreditRegistry, ());
        let registry_client = carbonchain_credit_registry::CreditRegistryClient::new(env, &registry_id);

        let admin = Address::generate(env);
        let verifier = Address::generate(env);
        let issuer = Address::generate(env);
        let retirement = Address::generate(env);
        registry_client.initialize(&admin, &retirement);
        registry_client.register_verifier(&admin, &verifier);

        let inonce = registry_client.nonce(&issuer);
        let credit_id = registry_client.submit_credit(
            &issuer,
            &String::from_str(env, "PROJ-001"),
            &2024,
            &String::from_str(env, "VCS"),
            &String::from_str(env, "NG"),
            &1_000_000,
            &String::from_str(env, "bafybei123"),
            &inonce,
        );
        let vnonce = registry_client.nonce(&verifier);
        registry_client.approve_and_mint(&verifier, &credit_id, &vnonce);

        let marketplace_id = env.register(Marketplace, ());
        let client = MarketplaceClient::new(env, &marketplace_id);
        let mp_admin = Address::generate(env);
        client.initialize(&mp_admin);
        let seller = Address::generate(env);
        (client, seller, mp_admin, registry_id, credit_id)
    }

    #[test]
    fn test_create_offer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        assert_eq!(offer_id, 0);
        let offer = client.get_offer(&offer_id);
        assert!(offer.active);
        assert_eq!(offer.price_xlm, 10_000_000);
    }

    #[test]
    fn test_cancel_offer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        client.cancel_offer(&seller, &offer_id);
        assert!(!client.get_offer(&offer_id).active);
    }

    #[test]
    fn test_cancel_already_closed_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        client.cancel_offer(&seller, &offer_id);
        assert!(client.try_cancel_offer(&seller, &offer_id).is_err());
    }

    #[test]
    fn test_invalid_price_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        assert!(client.try_create_offer(&seller, &credit_id, &0, &500_000, &registry_id).is_err());
    }

    #[test]
    fn test_get_offers_by_seller() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        client.create_offer(&seller, &credit_id, &20_000_000, &250_000, &registry_id);
        assert_eq!(client.get_offers_by_seller(&seller).len(), 2);
    }

    #[test]
    fn test_offer_count() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        client.create_offer(&seller, &credit_id, &20_000_000, &250_000, &registry_id);
        assert_eq!(client.offer_count(), 2);
    }

    #[test]
    fn test_unauthorized_cancel_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        let other = Address::generate(&env);
        let ononce = client.nonce(&other);
        assert!(client.try_cancel_offer(&other, &offer_id, &ononce).is_err());
    }

    // ── get_active_offers_by_seller tests ────────────────────────────────────

    #[test]
    fn test_get_active_offers_by_seller_filters_cancelled() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        let id0 = client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        let id1 = client.create_offer(&seller, &credit_id, &20_000_000, &250_000, &registry_id);
        // Cancel the first offer.
        client.cancel_offer(&seller, &id0);
        // get_offers_by_seller still returns both.
        assert_eq!(client.get_offers_by_seller(&seller).len(), 2);
        // get_active_offers_by_seller must return only the open one.
        let active = client.get_active_offers_by_seller(&seller);
        assert_eq!(active.len(), 1);
        assert_eq!(active.get(0).unwrap(), id1);
    }

    #[test]
    fn test_get_active_offers_by_seller_empty_when_all_cancelled() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, _admin, registry_id, credit_id) = setup_with_registry(&env);
        let id0 = client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        client.cancel_offer(&seller, &id0);
        assert_eq!(client.get_active_offers_by_seller(&seller).len(), 0);
    }

    // ── Pause tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_pause_blocks_create_offer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, admin, registry_id, credit_id) = setup_with_registry(&env);
        client.pause(&admin);
        assert!(client.paused());
        assert!(client.try_create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id).is_err());
    }

    #[test]
    fn test_unpause_restores_create_offer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, admin, registry_id, credit_id) = setup_with_registry(&env);
        client.pause(&admin);
        client.unpause(&admin);
        assert!(!client.paused());
        assert!(client.try_create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id).is_ok());
    }

    #[test]
    fn test_pause_blocks_cancel_offer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, seller, admin, registry_id, credit_id) = setup_with_registry(&env);
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000, &registry_id);
        client.pause(&admin);
        assert!(client.try_cancel_offer(&seller, &offer_id).is_err());
    }

    #[test]
    fn test_non_admin_cannot_pause() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, _, _, _) = setup_with_registry(&env);
        let rando = Address::generate(&env);
        assert!(client.try_pause(&rando).is_err());
    }
}
