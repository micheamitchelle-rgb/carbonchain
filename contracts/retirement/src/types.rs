use soroban_sdk::{contracttype, Address, String, BytesN};

/// Minimum TTL in ledgers (~1 year at 5s/ledger).
pub const MIN_TTL: u32 = 6_307_200;
/// Threshold below which TTL is extended (half of MIN_TTL).
pub const TTL_THRESHOLD: u32 = MIN_TTL / 2;

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct RetirementRecord {
    pub credit_id: BytesN<32>,
    pub buyer: Address,
    pub tonnes_retired: i128,
    pub reason: String,
    pub retired_at: u64,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Retirement(BytesN<32>),
    AccountRetirements(Address),
    Admin,
    Paused,
}
