//! RPC methods related to mining only available with `getblocktemplate-rpcs` rust feature.

use std::sync::Arc;

use futures::{FutureExt, TryFutureExt};
use jsonrpc_core::{self, BoxFuture, Error, ErrorCode, Result};
use jsonrpc_derive::rpc;
use tower::{buffer::Buffer, Service, ServiceExt};

use zebra_chain::{
    amount::Amount,
    block::Height,
    block::{self, Block},
    chain_tip::ChainTip,
    serialization::ZcashDeserializeInto,
};
use zebra_consensus::{BlockError, VerifyBlockError, VerifyChainError, VerifyCheckpointError};
use zebra_node_services::mempool;

use crate::methods::{
    best_chain_tip_height,
    get_block_template_rpcs::types::{
        default_roots::DefaultRoots, get_block_template::GetBlockTemplate, hex_data::HexData,
        submit_block, transaction::TransactionTemplate,
    },
    GetBlockHash, MISSING_BLOCK_ERROR_CODE,
};

pub mod config;
pub(crate) mod types;

/// getblocktemplate RPC method signatures.
#[rpc(server)]
pub trait GetBlockTemplateRpc {
    /// Returns the height of the most recent block in the best valid block chain (equivalently,
    /// the number of blocks in this chain excluding the genesis block).
    ///
    /// zcashd reference: [`getblockcount`](https://zcash.github.io/rpc/getblockcount.html)
    ///
    /// # Notes
    ///
    /// This rpc method is available only if zebra is built with `--features getblocktemplate-rpcs`.
    #[rpc(name = "getblockcount")]
    fn get_block_count(&self) -> Result<u32>;

    /// Returns the hash of the block of a given height iff the index argument correspond
    /// to a block in the best chain.
    ///
    /// zcashd reference: [`getblockhash`](https://zcash-rpc.github.io/getblockhash.html)
    ///
    /// # Parameters
    ///
    /// - `index`: (numeric, required) The block index.
    ///
    /// # Notes
    ///
    /// - If `index` is positive then index = block height.
    /// - If `index` is negative then -1 is the last known valid block.
    /// - This rpc method is available only if zebra is built with `--features getblocktemplate-rpcs`.
    #[rpc(name = "getblockhash")]
    fn get_block_hash(&self, index: i32) -> BoxFuture<Result<GetBlockHash>>;

    /// Returns a block template for mining new Zcash blocks.
    ///
    /// # Parameters
    ///
    /// - `jsonrequestobject`: (string, optional) A JSON object containing arguments.
    ///
    /// zcashd reference: [`getblocktemplate`](https://zcash-rpc.github.io/getblocktemplate.html)
    ///
    /// # Notes
    ///
    /// Arguments to this RPC are currently ignored.
    /// Long polling, block proposals, server lists, and work IDs are not supported.
    ///
    /// Miners can make arbitrary changes to blocks, as long as:
    /// - the data sent to `submitblock` is a valid Zcash block, and
    /// - the parent block is a valid block that Zebra already has, or will receive soon.
    ///
    /// Zebra verifies blocks in parallel, and keeps recent chains in parallel,
    /// so moving between chains is very cheap. (But forking a new chain may take some time,
    /// until bug #4794 is fixed.)
    ///
    /// This rpc method is available only if zebra is built with `--features getblocktemplate-rpcs`.
    #[rpc(name = "getblocktemplate")]
    fn get_block_template(&self) -> BoxFuture<Result<GetBlockTemplate>>;

    /// Submits block to the node to be validated and committed.
    /// Returns the [`submit_block::Response`] for the operation, as a JSON string.
    ///
    /// zcashd reference: [`submitblock`](https://zcash.github.io/rpc/submitblock.html)
    ///
    /// # Parameters
    /// - `hexdata` (string, required)
    /// - `jsonparametersobject` (string, optional) - currently ignored
    ///  - holds a single field, workid, that must be included in submissions if provided by the server.
    #[rpc(name = "submitblock")]
    fn submit_block(
        &self,
        hex_data: HexData,
        _options: Option<submit_block::JsonParameters>,
    ) -> BoxFuture<Result<submit_block::Response>>;
}

/// RPC method implementations.
pub struct GetBlockTemplateRpcImpl<Mempool, State, Tip, ChainVerifier>
where
    Mempool: Service<
        mempool::Request,
        Response = mempool::Response,
        Error = zebra_node_services::BoxError,
    >,
    State: Service<
        zebra_state::ReadRequest,
        Response = zebra_state::ReadResponse,
        Error = zebra_state::BoxError,
    >,
    ChainVerifier: Service<Arc<Block>, Response = block::Hash, Error = zebra_consensus::BoxError>
        + Clone
        + Send
        + Sync
        + 'static,
{
    // TODO: Add the other fields from the [`Rpc`] struct as-needed

    // Configuration
    //
    // TODO: add mining config for getblocktemplate RPC miner address

    // Services
    //
    /// A handle to the mempool service.
    mempool: Buffer<Mempool, mempool::Request>,

    /// A handle to the state service.
    state: State,

    /// Allows efficient access to the best tip of the blockchain.
    latest_chain_tip: Tip,

    /// The chain verifier, used for submitting blocks.
    chain_verifier: ChainVerifier,
}

impl<Mempool, State, Tip, ChainVerifier> GetBlockTemplateRpcImpl<Mempool, State, Tip, ChainVerifier>
where
    Mempool: Service<
            mempool::Request,
            Response = mempool::Response,
            Error = zebra_node_services::BoxError,
        > + 'static,
    State: Service<
            zebra_state::ReadRequest,
            Response = zebra_state::ReadResponse,
            Error = zebra_state::BoxError,
        > + Clone
        + Send
        + Sync
        + 'static,
    Tip: ChainTip + Clone + Send + Sync + 'static,
    ChainVerifier: Service<Arc<Block>, Response = block::Hash, Error = zebra_consensus::BoxError>
        + Clone
        + Send
        + Sync
        + 'static,
{
    /// Create a new instance of the handler for getblocktemplate RPCs.
    pub fn new(
        mempool: Buffer<Mempool, mempool::Request>,
        state: State,
        latest_chain_tip: Tip,
        chain_verifier: ChainVerifier,
    ) -> Self {
        Self {
            mempool,
            state,
            latest_chain_tip,
            chain_verifier,
        }
    }
}

impl<Mempool, State, Tip, ChainVerifier> GetBlockTemplateRpc
    for GetBlockTemplateRpcImpl<Mempool, State, Tip, ChainVerifier>
where
    Mempool: Service<
            mempool::Request,
            Response = mempool::Response,
            Error = zebra_node_services::BoxError,
        > + 'static,
    Mempool::Future: Send,
    State: Service<
            zebra_state::ReadRequest,
            Response = zebra_state::ReadResponse,
            Error = zebra_state::BoxError,
        > + Clone
        + Send
        + Sync
        + 'static,
    <State as Service<zebra_state::ReadRequest>>::Future: Send,
    Tip: ChainTip + Clone + Send + Sync + 'static,
    ChainVerifier: Service<Arc<Block>, Response = block::Hash, Error = zebra_consensus::BoxError>
        + Clone
        + Send
        + Sync
        + 'static,
    <ChainVerifier as Service<Arc<Block>>>::Future: Send,
{
    fn get_block_count(&self) -> Result<u32> {
        best_chain_tip_height(&self.latest_chain_tip).map(|height| height.0)
    }

    fn get_block_hash(&self, index: i32) -> BoxFuture<Result<GetBlockHash>> {
        let mut state = self.state.clone();
        let latest_chain_tip = self.latest_chain_tip.clone();

        async move {
            let tip_height = best_chain_tip_height(&latest_chain_tip)?;

            let height = get_height_from_int(index, tip_height)?;

            let request = zebra_state::ReadRequest::BestChainBlockHash(height);
            let response = state
                .ready()
                .and_then(|service| service.call(request))
                .await
                .map_err(|error| Error {
                    code: ErrorCode::ServerError(0),
                    message: error.to_string(),
                    data: None,
                })?;

            match response {
                zebra_state::ReadResponse::BlockHash(Some(hash)) => Ok(GetBlockHash(hash)),
                zebra_state::ReadResponse::BlockHash(None) => Err(Error {
                    code: MISSING_BLOCK_ERROR_CODE,
                    message: "Block not found".to_string(),
                    data: None,
                }),
                _ => unreachable!("unmatched response to a block request"),
            }
        }
        .boxed()
    }

    fn get_block_template(&self) -> BoxFuture<Result<GetBlockTemplate>> {
        let mempool = self.mempool.clone();
        let latest_chain_tip = self.latest_chain_tip.clone();

        // Since this is a very large RPC, we use separate functions for each group of fields.
        async move {
            let _tip_height = best_chain_tip_height(&latest_chain_tip)?;

            // TODO: put this in a separate get_mempool_transactions() function
            let request = mempool::Request::FullTransactions;
            let response = mempool.oneshot(request).await.map_err(|error| Error {
                code: ErrorCode::ServerError(0),
                message: error.to_string(),
                data: None,
            })?;

            let transactions = if let mempool::Response::FullTransactions(transactions) = response {
                // TODO: select transactions according to ZIP-317 (#5473)
                transactions
            } else {
                unreachable!("unmatched response to a mempool::FullTransactions request");
            };

            let merkle_root;
            let auth_data_root;

            // TODO: add the coinbase transaction to these lists, and delete the is_empty() check
            if !transactions.is_empty() {
                merkle_root = transactions.iter().cloned().collect();
                auth_data_root = transactions.iter().cloned().collect();
            } else {
                merkle_root = [0; 32].into();
                auth_data_root = [0; 32].into();
            }

            let transactions = transactions.iter().map(Into::into).collect();

            let empty_string = String::from("");
            Ok(GetBlockTemplate {
                capabilities: vec![],

                version: 0,

                previous_block_hash: GetBlockHash([0; 32].into()),
                block_commitments_hash: [0; 32].into(),
                light_client_root_hash: [0; 32].into(),
                final_sapling_root_hash: [0; 32].into(),
                default_roots: DefaultRoots {
                    merkle_root,
                    chain_history_root: [0; 32].into(),
                    auth_data_root,
                    block_commitments_hash: [0; 32].into(),
                },

                transactions,

                // TODO: move to a separate function in the transactions module
                coinbase_txn: TransactionTemplate {
                    // TODO: generate coinbase transaction data
                    data: vec![].into(),

                    // TODO: calculate from transaction data
                    hash: [0; 32].into(),
                    auth_digest: [0; 32].into(),

                    // Always empty for coinbase transactions.
                    depends: Vec::new(),

                    // TODO: negative sum of transactions.*.fee
                    fee: Amount::zero(),

                    // TODO: sigops used by the generated transaction data
                    sigops: 0,

                    required: true,
                },

                target: empty_string.clone(),

                min_time: 0,

                mutable: vec![],

                nonce_range: empty_string.clone(),

                sigop_limit: 0,
                size_limit: 0,

                cur_time: 0,

                bits: empty_string,

                height: 0,
            })
        }
        .boxed()
    }

    fn submit_block(
        &self,
        HexData(block_bytes): HexData,
        _options: Option<submit_block::JsonParameters>,
    ) -> BoxFuture<Result<submit_block::Response>> {
        let mut chain_verifier = self.chain_verifier.clone();

        async move {
            let block: Block = match block_bytes.zcash_deserialize_into() {
                Ok(block_bytes) => block_bytes,
                Err(_) => return Ok(submit_block::ErrorResponse::Rejected.into()),
            };

            let chain_verifier_response = chain_verifier
                .ready()
                .await
                .map_err(|error| Error {
                    code: ErrorCode::ServerError(0),
                    message: error.to_string(),
                    data: None,
                })?
                .call(Arc::new(block))
                .await;

            let chain_error = match chain_verifier_response {
                // Currently, this match arm returns `null` (Accepted) for blocks committed
                // to any chain, but Accepted is only for blocks in the best chain.
                //
                // TODO (#5487):
                // - Inconclusive: check if the block is on a side-chain
                // The difference is important to miners, because they want to mine on the best chain.
                Ok(_block_hash) => return Ok(submit_block::Response::Accepted),

                // Turns BoxError into Result<VerifyChainError, BoxError>,
                // by downcasting from Any to VerifyChainError.
                Err(box_error) => box_error
                    .downcast::<VerifyChainError>()
                    .map(|boxed_chain_error| *boxed_chain_error),
            };

            Ok(match chain_error {
                Ok(
                    VerifyChainError::Checkpoint(VerifyCheckpointError::AlreadyVerified { .. })
                    | VerifyChainError::Block(VerifyBlockError::Block {
                        source: BlockError::AlreadyInChain(..),
                    }),
                ) => submit_block::ErrorResponse::Duplicate,

                // Currently, these match arms return Reject for the older duplicate in a queue,
                // but queued duplicates should be DuplicateInconclusive.
                //
                // Optional TODO (#5487):
                // - DuplicateInconclusive: turn these non-finalized state duplicate block errors
                //   into BlockError enum variants, and handle them as DuplicateInconclusive:
                //   - "block already sent to be committed to the state"
                //   - "replaced by newer request"
                // - keep the older request in the queue,
                //   and return a duplicate error for the newer request immediately.
                //   This improves the speed of the RPC response.
                //
                // Checking the download queues and ChainVerifier buffer for duplicates
                // might require architectural changes to Zebra, so we should only do it
                // if mining pools really need it.
                Ok(_verify_chain_error) => submit_block::ErrorResponse::Rejected,

                // This match arm is currently unreachable, but if future changes add extra error types,
                // we want to turn them into `Rejected`.
                Err(_unknown_error_type) => submit_block::ErrorResponse::Rejected,
            }
            .into())
        }
        .boxed()
    }
}

/// Given a potentially negative index, find the corresponding `Height`.
///
/// This function is used to parse the integer index argument of `get_block_hash`.
fn get_height_from_int(index: i32, tip_height: Height) -> Result<Height> {
    if index >= 0 {
        let height = index.try_into().expect("Positive i32 always fits in u32");
        if height > tip_height.0 {
            return Err(Error::invalid_params(
                "Provided index is greater than the current tip",
            ));
        }
        Ok(Height(height))
    } else {
        // `index + 1` can't overflow, because `index` is always negative here.
        let height = i32::try_from(tip_height.0)
            .expect("tip height fits in i32, because Height::MAX fits in i32")
            .checked_add(index + 1);

        let sanitized_height = match height {
            None => return Err(Error::invalid_params("Provided index is not valid")),
            Some(h) => {
                if h < 0 {
                    return Err(Error::invalid_params(
                        "Provided negative index ends up with a negative height",
                    ));
                }
                let h: u32 = h.try_into().expect("Positive i32 always fits in u32");
                if h > tip_height.0 {
                    return Err(Error::invalid_params(
                        "Provided index is greater than the current tip",
                    ));
                }

                h
            }
        };

        Ok(Height(sanitized_height))
    }
}