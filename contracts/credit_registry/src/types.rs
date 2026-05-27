use soroban_sdk::{contracttype, Address, String, BytesN, Vec};

#[derive(Clone, Copy, Debug, PartialEq)]
#[contracttype]
pub enum CreditStatus {
    Pending = 0,
    Active = 1,
    Retired = 2,
    Flagged = 3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[contracttype]
pub enum ServiceType {
    CreditApproval = 0,
    MRVReview = 1,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct CreditMetadata {
    pub project_id: String,
    pub issuer: Address,
    pub owner: Address,
    pub vintage_year: u32,
    pub methodology: String,
    pub geography: String,
    pub tonnes: i128,
    pub ipfs_hash: String,
    pub status: CreditStatus,
    pub issued_at: u64,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Admin,
    VerifierSet,
    Credit(BytesN<32>),
    ProjectCredits(String),
    RetirementContract,
    CreditNonce,
    Paused,
    VerifierServices(Address),
    PendingAdmin,
    Nonce(Address),
}
