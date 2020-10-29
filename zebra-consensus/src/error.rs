//! Errors that can occur when checking consensus rules.
//!
//! Each error variant corresponds to a consensus rule, so enumerating
//! all possible verification failures enumerates the consensus rules we
//! implement, and ensures that we don't reject blocks or transactions
//! for a non-enumerated reason.

use thiserror::Error;

use zebra_chain::primitives::ed25519;

use crate::BoxError;

#[derive(Error, Debug, PartialEq)]
pub enum SubsidyError {
    #[error("no coinbase transaction in block")]
    NoCoinbase,

    #[error("founders reward output not found")]
    FoundersRewardNotFound,
}

#[derive(Error, Debug, PartialEq)]
pub enum TransactionError {
    #[error("first transaction must be coinbase")]
    CoinbasePosition,

    #[error("coinbase input found in non-coinbase transaction")]
    CoinbaseInputFound,

    #[error("coinbase transaction MUST NOT have any JoinSplit descriptions or Spend descriptions")]
    CoinbaseHasJoinSplitOrSpend,

    #[error("coinbase transaction failed subsidy validation")]
    Subsidy(#[from] SubsidyError),

    #[error("transaction version number MUST be >= 4")]
    WrongVersion,

    #[error("at least one of tx_in_count, nShieldedSpend, and nJoinSplit MUST be nonzero")]
    NoTransfer,

    #[error("if there are no Spends or Outputs, the value balance MUST be 0.")]
    BadBalance,

    #[error("could not verify a transparent script")]
    Script(#[from] zebra_script::Error),

    // XXX change this when we align groth16 verifier errors with bellman
    // and add a from annotation when the error type is more precise
    #[error("spend proof MUST be valid given a primary input formed from the other fields except spendAuthSig")]
    Groth16,

    #[error(
        "joinSplitSig MUST represent a valid signature under joinSplitPubKey of dataToBeSigned"
    )]
    Ed25519(#[from] ed25519::Error),

    #[error("bindingSig MUST represent a valid signature under the transaction binding validating key bvk of SigHash")]
    RedJubjub(redjubjub::Error),
}

impl From<BoxError> for TransactionError {
    fn from(err: BoxError) -> Self {
        match err.downcast::<redjubjub::Error>() {
            Ok(e) => TransactionError::RedJubjub(*e),
            Err(e) => panic!(e),
        }
    }
}

impl From<SubsidyError> for BlockError {
    fn from(err: SubsidyError) -> BlockError {
        BlockError::Transaction(TransactionError::Subsidy(err))
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum BlockError {
    #[error("block contains invalid transactions")]
    Transaction(#[from] TransactionError),

    #[error("block haves no transactions")]
    NoTransactions,

    #[error("block {0:?} is already in the chain at depth {1:?}")]
    AlreadyInChain(zebra_chain::block::Hash, u32),

    #[error("invalid block {0:?}: missing block height")]
    MissingHeight(zebra_chain::block::Hash),

    #[error("invalid block height {0:?} in {1:?}: greater than the maximum height {2:?}")]
    MaxHeight(
        zebra_chain::block::Height,
        zebra_chain::block::Hash,
        zebra_chain::block::Height,
    ),

    #[error("invalid difficulty threshold in block header {0:?} {1:?}")]
    InvalidDifficulty(zebra_chain::block::Height, zebra_chain::block::Hash),

    #[error("block {0:?} has a difficulty threshold {2:?} that is easier than the {3:?} difficulty limit {4:?}, hash: {1:?}")]
    TargetDifficultyLimit(
        zebra_chain::block::Height,
        zebra_chain::block::Hash,
        zebra_chain::work::difficulty::ExpandedDifficulty,
        zebra_chain::parameters::Network,
        zebra_chain::work::difficulty::ExpandedDifficulty,
    ),

    #[error("block {0:?} has a hash {1:?} that is easier than the difficulty threshold {2:?}")]
    DifficultyFilter(
        zebra_chain::block::Height,
        zebra_chain::block::Hash,
        zebra_chain::work::difficulty::ExpandedDifficulty,
    ),
}
