#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, symbol_short,
    Env, Address, String, Vec,
};

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct MrvDataPoint {
    pub oracle: Address,
    pub project_id: String,
    pub tonnes: i128,
    pub recorded_at: u64,
    /// Flagged when the reading deviates >20% from the previous reading.
    pub anomaly: bool,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    /// Admin address allowed to register oracles.
    Admin,
    /// Set of authorised oracle addresses.
    OracleSet,
    /// Latest reading per project.
    Latest(String),
    /// Full history per project (Vec<MrvDataPoint>).
    History(String),
    /// Pause flag.
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum OracleError {
    NotInitialized     = 119,
    Unauthorized       = 120,
    AlreadyInitialized = 121,
    Overflow           = 122,
    ContractPaused     = 123,
}

// Maximum MRV history entries retained per project (ring-buffer eviction).
const MAX_HISTORY: u32 = 100;

/// Minimum TTL in ledgers (~1 year at 5s/ledger).
const MIN_TTL: u32 = 6_307_200;
/// Threshold below which TTL is extended.
const TTL_THRESHOLD: u32 = MIN_TTL / 2;

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct MrvOracle;

#[contractimpl]
impl MrvOracle {
    pub fn initialize(env: Env, admin: Address) -> Result<(), OracleError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(OracleError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.events().publish((symbol_short!("mrv_init"),), admin);
        Ok(())
    }

    // ── Pause / Unpause ──────────────────────────────────────────────────────

    pub fn pause(env: Env, admin: Address) -> Result<(), OracleError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((symbol_short!("paused"),), admin);
        Ok(())
    }

    pub fn unpause(env: Env, admin: Address) -> Result<(), OracleError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((symbol_short!("unpaused"),), admin);
        Ok(())
    }

    pub fn paused(env: Env) -> bool {
        env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
    }

    // ── Oracle management ────────────────────────────────────────────────────

    /// Register an oracle address. Returns `true` if newly added, `false` if
    /// already registered. Emits `oracle_new` only on first registration and
    /// `oracle_dup` when the oracle was already present, so callers can
    /// distinguish the two cases from on-chain events.
    pub fn register_oracle(env: Env, admin: Address, oracle: Address) -> Result<bool, OracleError> {
        Self::require_admin(&env, &admin)?;
        if !Self::consume_nonce(&env, &admin, nonce) {
            return Err(OracleError::InvalidNonce);
        }
        let mut set: Vec<Address> = env
            .storage().instance()
            .get(&DataKey::OracleSet)
            .unwrap_or_else(|| Vec::new(&env));
        if set.contains(&oracle) {
            // Already registered — emit a distinct event so callers know.
            env.events().publish((symbol_short!("orc_dup"),), oracle);
            return Ok(false);
        }
        set.push_back(oracle.clone());
        env.storage().instance().set(&DataKey::OracleSet, &set);
        env.events().publish((symbol_short!("orc_new"),), oracle);
        Ok(true)
    }

    /// Submit a new MRV reading for a project.
    /// Anomaly flag is set when the new reading deviates >20% from the previous one.
    pub fn update_mrv_data(
        env: Env,
        oracle: Address,
        project_id: String,
        tonnes: i128,
        nonce: u64,
    ) -> Result<bool, OracleError> {
        if Self::is_paused(&env) {
            return Err(OracleError::ContractPaused);
        }
        oracle.require_auth();
        if !Self::is_oracle(&env, &oracle) {
            return Err(OracleError::Unauthorized);
        }
        if !Self::consume_nonce(&env, &oracle, nonce) {
            return Err(OracleError::InvalidNonce);
        }

        let anomaly = Self::detect_anomaly(&env, &project_id, tonnes)?;

        let point = MrvDataPoint {
            oracle: oracle.clone(),
            project_id: project_id.clone(),
            tonnes,
            recorded_at: env.ledger().timestamp(),
            anomaly,
        };

        env.storage().persistent().set(&DataKey::Latest(project_id.clone()), &point);
        env.storage().persistent().extend_ttl(&DataKey::Latest(project_id.clone()), TTL_THRESHOLD, MIN_TTL);

        let hist_key = DataKey::History(project_id.clone());
        let mut history: Vec<MrvDataPoint> = env
            .storage().persistent()
            .get(&hist_key)
            .unwrap_or_else(|| Vec::new(&env));
        if history.len() >= MAX_HISTORY {
            // Evict oldest entry (index 0) to keep the ring buffer bounded.
            history.remove(0);
        }
        history.push_back(point);
        env.storage().persistent().set(&hist_key, &history);
        env.storage().persistent().extend_ttl(&hist_key, TTL_THRESHOLD, MIN_TTL);

        env.events().publish(
            (symbol_short!("mrv_upd"), oracle),
            (project_id, tonnes, anomaly),
        );

        Ok(anomaly)
    }

    pub fn get_latest(env: Env, project_id: String) -> Option<MrvDataPoint> {
        env.storage().persistent().get(&DataKey::Latest(project_id))
    }

    pub fn get_nonce(env: Env, address: Address) -> u64 {
        env.storage().persistent().get(&DataKey::Nonce(address)).unwrap_or(0u64)
    }

    pub fn propose_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), OracleError> {
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::PendingAdmin, &new_admin);
        Ok(())
    }

    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), OracleError> {
        let pending: Address = env.storage().instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(OracleError::NoPendingAdmin)?;
        if new_admin != pending {
            return Err(OracleError::Unauthorized);
        }
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        Ok(())
    }

    pub fn get_history(env: Env, project_id: String) -> Vec<MrvDataPoint> {
        env.storage()
            .persistent()
            .get(&DataKey::History(project_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), OracleError> {
        let admin: Address = env
            .storage().instance()
            .get(&DataKey::Admin)
            .ok_or(OracleError::NotInitialized)?;
        caller.require_auth();
        if *caller != admin {
            return Err(OracleError::Unauthorized);
        }
        Ok(())
    }

    fn consume_nonce(env: &Env, addr: &Address, expected: u64) -> bool {
        let current: u64 = env.storage().persistent()
            .get(&DataKey::Nonce(addr.clone())).unwrap_or(0u64);
        if current != expected { return false; }
        let key = DataKey::Nonce(addr.clone());
        env.storage().persistent().set(&key, &(current + 1));
        env.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, MIN_TTL);
        true
    }

    fn is_oracle(env: &Env, oracle: &Address) -> bool {
        let set: Vec<Address> = env
            .storage().instance()
            .get(&DataKey::OracleSet)
            .unwrap_or_else(|| Vec::new(env));
        set.contains(oracle)
    }

    fn is_paused(env: &Env) -> bool {
        env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
    }

    /// Returns true if `new_tonnes` deviates more than 20% from the last reading.
    fn detect_anomaly(env: &Env, project_id: &String, new_tonnes: i128) -> Result<bool, OracleError> {
        let prev: Option<MrvDataPoint> = env
            .storage().persistent()
            .get(&DataKey::Latest(project_id.clone()));
        match prev {
            None => Ok(false),
            Some(p) if p.tonnes == 0 => Ok(false),
            Some(p) => {
                let diff = (new_tonnes - p.tonnes).abs();
                // diff / prev > 0.20  ⟺  diff * 5 > prev
                let diff_times_5 = diff.checked_mul(5).ok_or(OracleError::Overflow)?;
                Ok(diff_times_5 > p.tonnes.abs())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Env, String};

    fn setup() -> (Env, MrvOracleClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(MrvOracle, ());
        let client = MrvOracleClient::new(&env, &id);
        let admin = Address::generate(&env);
        let oracle = Address::generate(&env);
        client.initialize(&admin);
        let nonce = client.get_nonce(&admin);
        client.register_oracle(&admin, &oracle, &nonce);
        (env, client, admin, oracle)
    }

    #[test]
    fn test_initialize_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(MrvOracle, ());
        let client = MrvOracleClient::new(&env, &id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        let events = env.events().all();
        // Exactly one event must be emitted: the mrv_init event.
        assert_eq!(events.len(), 1);
        let (_, topics, _data): (_, soroban_sdk::Vec<soroban_sdk::Val>, soroban_sdk::Val) =
            events.get(0).unwrap();
        // First topic is the symbol "mrv_init".
        let expected: soroban_sdk::Val = symbol_short!("mrv_init").into();
        assert_eq!(topics.get(0).unwrap(), expected);
    }

    #[test]
    fn test_update_and_get_latest() {
        let (env, client, _admin, oracle) = setup();
        let proj = String::from_str(&env, "PROJ-001");
        let nonce = client.get_nonce(&oracle);
        client.update_mrv_data(&oracle, &proj, &1_000_000, &nonce);
        let latest = client.get_latest(&proj).unwrap();
        assert_eq!(latest.tonnes, 1_000_000);
        assert!(!latest.anomaly);
    }

    #[test]
    fn test_history_accumulates() {
        let (env, client, _admin, oracle) = setup();
        let proj = String::from_str(&env, "PROJ-001");
        let nonce = client.get_nonce(&oracle);
        client.update_mrv_data(&oracle, &proj, &1_000_000, &nonce);
        let nonce2 = client.get_nonce(&oracle);
        client.update_mrv_data(&oracle, &proj, &1_050_000, &nonce2);
        assert_eq!(client.get_history(&proj).len(), 2);
    }

    #[test]
    fn test_anomaly_flagged_on_large_deviation() {
        let (env, client, _admin, oracle) = setup();
        let proj = String::from_str(&env, "PROJ-001");
        let nonce = client.get_nonce(&oracle);
        client.update_mrv_data(&oracle, &proj, &1_000_000, &nonce);
        let nonce2 = client.get_nonce(&oracle);
        let anomaly = client.update_mrv_data(&oracle, &proj, &1_500_000, &nonce2);
        assert!(anomaly);
        assert!(client.get_latest(&proj).unwrap().anomaly);
    }

    #[test]
    fn test_no_anomaly_on_small_deviation() {
        let (env, client, _admin, oracle) = setup();
        let proj = String::from_str(&env, "PROJ-001");
        let nonce = client.get_nonce(&oracle);
        client.update_mrv_data(&oracle, &proj, &1_000_000, &nonce);
        let nonce2 = client.get_nonce(&oracle);
        let anomaly = client.update_mrv_data(&oracle, &proj, &1_100_000, &nonce2);
        assert!(!anomaly);
    }

    #[test]
    fn test_unauthorized_oracle_rejected() {
        let (env, client, _admin, _oracle) = setup();
        let proj = String::from_str(&env, "PROJ-001");
        let rogue = Address::generate(&env);
        let nonce = client.get_nonce(&rogue);
        assert!(client.try_update_mrv_data(&rogue, &proj, &1_000_000, &nonce).is_err());
    }

    #[test]
    fn test_history_cap_evicts_oldest() {
        let (env, client, _admin, oracle) = setup();
        let proj = String::from_str(&env, "PROJ-CAP");
        for i in 0..=MAX_HISTORY {
            let nonce = client.get_nonce(&oracle);
            client.update_mrv_data(&oracle, &proj, &(i as i128 * 1_000), &nonce);
        }
        let history = client.get_history(&proj);
        assert_eq!(history.len(), MAX_HISTORY);
        assert_eq!(history.get(0).unwrap().tonnes, 1_000);
    }

    // ── register_oracle duplicate tests ─────────────────────────────────────

    #[test]
    fn test_register_oracle_returns_true_for_new() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(MrvOracle, ());
        let client = MrvOracleClient::new(&env, &id);
        let admin = Address::generate(&env);
        let oracle = Address::generate(&env);
        client.initialize(&admin);
        let newly_added = client.register_oracle(&admin, &oracle);
        assert!(newly_added);
    }

    #[test]
    fn test_register_oracle_returns_false_for_duplicate() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(MrvOracle, ());
        let client = MrvOracleClient::new(&env, &id);
        let admin = Address::generate(&env);
        let oracle = Address::generate(&env);
        client.initialize(&admin);
        client.register_oracle(&admin, &oracle);
        // Second registration of the same oracle must return false.
        let newly_added = client.register_oracle(&admin, &oracle);
        assert!(!newly_added);
    }

    #[test]
    fn test_register_oracle_duplicate_emits_oracle_dup_event() {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(MrvOracle, ());
        let client = MrvOracleClient::new(&env, &id);
        let admin = Address::generate(&env);
        let oracle = Address::generate(&env);
        client.initialize(&admin);
        client.register_oracle(&admin, &oracle);
        // Clear events so we only see the duplicate-registration event.
        let events_before = env.events().all().len();
        client.register_oracle(&admin, &oracle);
        let events_after = env.events().all();
        // One new event must have been emitted.
        assert_eq!(events_after.len(), events_before + 1);
        let (_, topics, _): (_, soroban_sdk::Vec<soroban_sdk::Val>, soroban_sdk::Val) =
            events_after.get(events_before).unwrap();
        let expected: soroban_sdk::Val = symbol_short!("orc_dup").into();
        assert_eq!(topics.get(0).unwrap(), expected);
    }

    // ── Pause tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_pause_blocks_update_mrv_data() {
        let (env, client, admin, oracle) = setup();
        client.pause(&admin);
        assert!(client.paused());
        let proj = String::from_str(&env, "PROJ-001");
        assert!(client.try_update_mrv_data(&oracle, &proj, &1_000_000).is_err());
    }

    #[test]
    fn test_unpause_restores_update_mrv_data() {
        let (env, client, admin, oracle) = setup();
        client.pause(&admin);
        client.unpause(&admin);
        assert!(!client.paused());
        let proj = String::from_str(&env, "PROJ-001");
        assert!(client.try_update_mrv_data(&oracle, &proj, &1_000_000).is_ok());
    }

    #[test]
    fn test_non_admin_cannot_pause() {
        let (env, client, _, _) = setup();
        let rando = Address::generate(&env);
        assert!(client.try_pause(&rando).is_err());
    }
}
