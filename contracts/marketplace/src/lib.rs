#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Env, Address, BytesN, Vec};

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
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MarketplaceError {
    OfferNotFound = 115,
    Unauthorized  = 116,
    InvalidPrice  = 117,
    AlreadyClosed = 118,
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct Marketplace;

#[contractimpl]
impl Marketplace {
    /// List a credit for sale. Returns the new offer ID.
    pub fn create_offer(
        env: Env,
        seller: Address,
        credit_id: BytesN<32>,
        price_xlm: i128,
        tonnes: i128,
    ) -> Result<u64, MarketplaceError> {
        seller.require_auth();
        if price_xlm <= 0 || tonnes <= 0 {
            return Err(MarketplaceError::InvalidPrice);
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

        // Index under seller
        let key = DataKey::SellerOffers(seller.clone());
        let mut ids: Vec<u64> = env.storage().persistent().get(&key).unwrap_or_else(|| Vec::new(&env));
        ids.push_back(offer_id);
        env.storage().persistent().set(&key, &ids);

        env.events().publish((symbol_short!("offer_new"), seller), offer_id);
        Ok(offer_id)
    }

    /// Cancel an open offer. Only the original seller may cancel.
    ///
    /// Emits an `offer_cxl` event **only** on success. Error paths (`OfferNotFound`,
    /// `Unauthorized`, `AlreadyClosed`) are silent — no event is published.
    pub fn cancel_offer(env: Env, seller: Address, offer_id: u64) -> Result<(), MarketplaceError> {
        seller.require_auth();
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
        env.events().publish((symbol_short!("offer_cxl"), seller), offer_id);
        Ok(())
    }

    pub fn get_offer(env: Env, offer_id: u64) -> Result<Offer, MarketplaceError> {
        env.storage()
            .persistent()
            .get(&DataKey::Offer(offer_id))
            .ok_or(MarketplaceError::OfferNotFound)
    }

    pub fn get_offers_by_seller(env: Env, seller: Address) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::SellerOffers(seller))
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn offer_count(env: Env) -> u64 {
        env.storage().persistent().get(&DataKey::OfferCount).unwrap_or(0u64)
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    fn next_id(env: &Env) -> u64 {
        let id: u64 = env.storage().persistent().get(&DataKey::OfferCount).unwrap_or(0u64);
        env.storage().persistent().set(&DataKey::OfferCount, &(id + 1));
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Env, BytesN};

    fn setup() -> (Env, MarketplaceClient<'static>, Address, BytesN<32>) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(Marketplace, ());
        let client = MarketplaceClient::new(&env, &id);
        let seller = Address::generate(&env);
        let credit_id = BytesN::from_array(&env, &[1u8; 32]);
        (env, client, seller, credit_id)
    }

    #[test]
    fn test_create_offer() {
        let (_env, client, seller, credit_id) = setup();
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000);
        assert_eq!(offer_id, 0);
        let offer = client.get_offer(&offer_id);
        assert!(offer.active);
        assert_eq!(offer.price_xlm, 10_000_000);
    }

    #[test]
    fn test_cancel_offer() {
        let (_env, client, seller, credit_id) = setup();
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000);
        client.cancel_offer(&seller, &offer_id);
        assert!(!client.get_offer(&offer_id).active);
    }

    #[test]
    fn test_cancel_already_closed_emits_no_event() {
        let (env, client, seller, credit_id) = setup();
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000);
        client.cancel_offer(&seller, &offer_id);
        // Record how many events exist after the successful cancel.
        let count_before = env.events().all().len();
        // The error path must not publish any additional event.
        let _ = client.try_cancel_offer(&seller, &offer_id);
        assert_eq!(env.events().all().len(), count_before);
    }

    #[test]
    fn test_cancel_already_closed_fails() {
        let (_env, client, seller, credit_id) = setup();
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000);
        client.cancel_offer(&seller, &offer_id);
        assert!(client.try_cancel_offer(&seller, &offer_id).is_err());
    }

    #[test]
    fn test_invalid_price_fails() {
        let (_env, client, seller, credit_id) = setup();
        assert!(client.try_create_offer(&seller, &credit_id, &0, &500_000).is_err());
    }

    #[test]
    fn test_get_offers_by_seller() {
        let (_env, client, seller, credit_id) = setup();
        client.create_offer(&seller, &credit_id, &10_000_000, &500_000);
        client.create_offer(&seller, &credit_id, &20_000_000, &250_000);
        assert_eq!(client.get_offers_by_seller(&seller).len(), 2);
    }

    #[test]
    fn test_offer_count() {
        let (_env, client, seller, credit_id) = setup();
        client.create_offer(&seller, &credit_id, &10_000_000, &500_000);
        client.create_offer(&seller, &credit_id, &20_000_000, &250_000);
        assert_eq!(client.offer_count(), 2);
    }

    #[test]
    fn test_unauthorized_cancel_fails() {
        let (env, client, seller, credit_id) = setup();
        let offer_id = client.create_offer(&seller, &credit_id, &10_000_000, &500_000);
        let other = Address::generate(&env);
        assert!(client.try_cancel_offer(&other, &offer_id).is_err());
    }

    #[test]
    fn test_offer_count_survives_contract_reinstantiation() {
        // Simulates an upgrade: the same contract address is re-registered (instance storage
        // is wiped) but persistent storage survives. OfferCount must still be correct.
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(Marketplace, ());
        let client = MarketplaceClient::new(&env, &contract_id);
        let seller = Address::generate(&env);
        let credit_id = BytesN::from_array(&env, &[1u8; 32]);

        client.create_offer(&seller, &credit_id, &10_000_000, &500_000);
        client.create_offer(&seller, &credit_id, &20_000_000, &250_000);
        assert_eq!(client.offer_count(), 2);

        // Re-register the same contract (simulates upgrade wiping instance storage)
        env.register_at(&contract_id, Marketplace, ());

        // Persistent storage survives — count must still be 2
        assert_eq!(client.offer_count(), 2);

        // Next offer ID must not collide with existing ones
        let new_offer_id = client.create_offer(&seller, &credit_id, &5_000_000, &100_000);
        assert_eq!(new_offer_id, 2);
    }
}
