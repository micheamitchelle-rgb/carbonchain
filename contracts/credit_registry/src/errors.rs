use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum CarbonChainError {
    NotInitialized = 100,
    AlreadyInitialized = 101,
    Unauthorized = 102,
    InvalidMetadata = 103,
    CreditNotFound = 104,
    InvalidStatusTransition = 105,
    VerifierAlreadyExists = 106,
    VerifierNotFound = 107,
    InsufficientBalance = 108,
    Overflow = 109,
    InvalidTonnes = 110,
    InvalidAdmin = 111,
    ContractPaused = 112,
}
