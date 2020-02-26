// Built-in deps
use std::collections::{HashSet, VecDeque};
use std::str::FromStr;
use std::time::Duration;
// External uses
use bigdecimal::BigDecimal;
use futures::channel::mpsc;
use tokio::runtime::Runtime;
use tokio::time;
use web3::contract::Options;
use web3::types::{H256, U256};
// Workspace uses
use eth_client::SignedCallResult;
use models::config_options::{ConfigurationOptions, ThreadPanicNotify};
use models::node::config;
use models::{Action, ActionType, Operation};
use storage::{ConnectionPool, StorageProcessor};
// Local uses
use self::ethereum_interface::{EthereumHttpClient, EthereumInterface};
pub use self::transactions::*;

mod ethereum_interface;
pub mod transactions;

const EXPECTED_WAIT_TIME_BLOCKS: u64 = 30;
const TX_POLL_PERIOD: Duration = Duration::from_secs(5);
const WAIT_CONFIRMATIONS: u64 = 1;

/// `ETHSender` is a structure capable of anchoring
/// the ZKSync operations to the Ethereum blockchain.
///
/// # Description
///
/// The essential part of this structure is an event loop (which is supposed to be run
/// in a separate thread), which obtains the operations to commit through the channel,
/// and then commits them to the Ethereum, ensuring that all the transactions are
/// successfully included in blocks and executed.
///
/// Also `ETHSender` preserves the order of operations: it guarantees that operations
/// are committed in FIFO order, meaning that until the older operation is committed
/// and has enough confirmations, no other operations will be committed.
///
/// # Transaction sending policy
///
/// The goal is to handle stuck transactions.
///
/// When we try to commit operation to ETH, we select nonce, gas price, sign
/// transaction and watch for its confirmations.
///
/// If transaction is not confirmed for a while, we increase the gas price and do the same, but we
/// keep the list of all sent transactions for one particular operations, since we can't be
/// sure which one will be committed; thus we have to track all of them.
///
/// Note: make sure to save signed tx to db before sending it to ETH, this way we can be sure
/// that state is always recoverable.
struct ETHSender<Eth: EthereumInterface> {
    /// Unconfirmed operations queue.
    unconfirmed_ops: VecDeque<OperationETHState>,
    /// Connection to the database.
    db_pool: ConnectionPool,
    /// Ethereum intermediator.
    ethereum: Eth,
    /// Channel for receiving operations to commit.
    rx_for_eth: mpsc::Receiver<Operation>,
    /// Channel to notify about committed operations.
    op_notify: mpsc::Sender<Operation>,
}

impl<Eth: EthereumInterface> ETHSender<Eth> {
    fn new(
        db_pool: ConnectionPool,
        ethereum: Eth,
        rx_for_eth: mpsc::Receiver<Operation>,
        op_notify: mpsc::Sender<Operation>,
    ) -> Self {
        let mut sender = Self {
            ethereum,
            unconfirmed_ops: VecDeque::new(),
            db_pool: db_pool.clone(),
            rx_for_eth,
            op_notify,
        };
        let storage = db_pool.access_storage().expect("Failed to access storage");
        if sender.restore_state(storage).is_err() {
            info!("No unconfirmed operations");
        }
        sender
    }

    async fn run(mut self) {
        let mut timer = time::interval(TX_POLL_PERIOD);

        loop {
            // Update the incoming operations.
            self.retrieve_operations();
            timer.tick().await;

            // Commit the next operation (if any).
            if let Some(current_op) = self.unconfirmed_ops.pop_front() {
                self.try_commit(current_op);
            }
        }
    }

    /// Obtains all the available operations to commit through the channel
    /// and stores them within self for further processing.
    fn retrieve_operations(&mut self) {
        while let Ok(Some(operation)) = self.rx_for_eth.try_next() {
            self.unconfirmed_ops.push_back(OperationETHState {
                operation,
                txs: Vec::new(),
            });
        }
    }

    /// Restores the state of `ETHSender` from the database.
    fn restore_state(&mut self, storage: StorageProcessor) -> Result<(), failure::Error> {
        self.unconfirmed_ops = storage
            .load_unconfirmed_operations()?
            .into_iter()
            .map(|(operation, txs)| OperationETHState {
                operation,
                txs: txs.into_iter().map(|tx| tx.into()).collect(),
            })
            .collect();
        Ok(())
    }

    /// Attempts to commit the provided operation to the Ethereum blockchain.
    ///
    /// The strategy is the following:
    /// - First we check the transactions associated with the operation.
    ///   If there are none, we create and send one, storing it locally. No more
    ///   processing at this step; we need to wait.
    ///   If there are some transactions, we check their state. If one of them
    ///   is committed and has enough approvals, we're all good.
    ///   Otherwise, we check if the last pending transaction is "stuck", meaning
    ///   that it is not being included in a block for a decent amount of time. If
    ///   so, we create a new transaction (with increased gas) and send it.
    /// - If there was no confirmation of a transaction in a previous step, we return
    ///   the operation to the beginning of the unprocessed operations queue. We will
    ///   check it again after some time.
    /// - If transaction was confirmed, there may be two possible outcomes:
    ///   1. Transaction is executed successfully. Desirable outcome, in which we
    ///      consider the commitment completed and notify about it through the channel.
    ///   2. Transaction erred. This should never happen, but if so, such an incident is
    ///      reported according to the chosen failure report policy.
    fn try_commit(&mut self, mut operation: OperationETHState) {
        // Check the transactions associated with the operation, and send a new one if required.

        // TODO is it correct to use `unwrap_or_default` here?
        let result = self
            .drive_to_completion(&mut operation)
            .map_err(|e| {
                warn!("Error while trying to complete uncommitted op: {}", e);
            })
            .unwrap_or_default();

        // Check if we've completed the commitment.
        match result {
            OperationCommitment::Committed => {
                info!(
                    "Operation {}, {}  block: {}, confirmed on ETH",
                    operation.operation.id.unwrap(),
                    operation.operation.action.to_string(),
                    operation.operation.block.block_number,
                );

                if operation.operation.action.get_type() == ActionType::VERIFY {
                    // We notify about verify only when commit is confirmed on the Ethereum.
                    self.op_notify
                        .try_send(operation.operation)
                        .map_err(|e| warn!("Failed notify about verify op confirmation: {}", e))
                        .unwrap_or_default();

                    // Complete pending withdrawals after each verify.
                    self.call_complete_withdrawals()
                        .map_err(|e| {
                            warn!("Error: {}", e);
                        })
                        .unwrap_or_default();
                }
            }
            OperationCommitment::Pending => {
                // Retry the operation again the next time.
                self.unconfirmed_ops.push_front(operation);
            }
        }
    }

    fn save_signed_tx_to_db(&self, tx: &TransactionETHState) -> Result<(), failure::Error> {
        let storage = self.db_pool.access_storage()?;
        Ok(storage.save_operation_eth_tx(
            tx.op_id,
            tx.signed_tx.hash,
            tx.deadline_block,
            tx.signed_tx.nonce.as_u32(),
            BigDecimal::from_str(&tx.signed_tx.gas_price.to_string()).unwrap(),
            tx.signed_tx.raw_tx.clone(),
        )?)
    }

    fn save_completed_tx_to_db(&self, hash: &H256) -> Result<(), failure::Error> {
        let storage = self.db_pool.access_storage()?;
        Ok(storage.confirm_eth_tx(hash)?)
    }

    fn check_transaction_state(
        &self,
        tx: &TransactionETHState,
        current_block: u64,
    ) -> Result<TxCheckOutcome, failure::Error> {
        let status = self.ethereum.get_tx_status(&tx.signed_tx.hash)?;

        let outcome = match status {
            // Successful execution.
            Some(status) if status.success => {
                // Check if transaction has enough confirmations.
                if status.confirmations >= WAIT_CONFIRMATIONS {
                    TxCheckOutcome::Committed
                } else {
                    TxCheckOutcome::Pending
                }
            }
            // Non-successful execution.
            Some(status) => {
                // Transaction failed, report the failure with details.

                // TODO check confirmations for fail
                assert!(
                    status.receipt.is_some(),
                    "Receipt should exist for a failed transaction"
                );
                TxCheckOutcome::Failed(status.receipt.unwrap())
            }
            // Stuck transaction.
            None if tx.is_stuck(current_block) => TxCheckOutcome::Stuck,
            // No status and not stuck yet, thus considered pending.
            None => TxCheckOutcome::Pending,
        };

        Ok(outcome)
    }

    fn drive_to_completion(
        &self,
        op: &mut OperationETHState,
    ) -> Result<OperationCommitment, failure::Error> {
        let current_block = self.ethereum.block_number()?;

        // Check statuses of existing transactions.
        let mut failed_txs: HashSet<H256> = HashSet::new();
        let mut last_stuck_tx: Option<&TransactionETHState> = None;

        // Go through every transaction in a loop. We will exit this method early
        // if there will be discovered a pending or successfully committed transaction.
        for tx in &op.txs {
            match self.check_transaction_state(tx, current_block)? {
                TxCheckOutcome::Pending => {
                    // Transaction is pending, nothing to do yet.
                    return Ok(OperationCommitment::Pending);
                }
                TxCheckOutcome::Committed => {
                    info!(
                        "Operation {}, {}  block: {}, committed, tx: {:#x}",
                        op.operation.id.unwrap(),
                        op.operation.action.to_string(),
                        op.operation.block.block_number,
                        tx.signed_tx.hash,
                    );
                    self.save_completed_tx_to_db(&tx.signed_tx.hash)?;
                    return Ok(OperationCommitment::Committed);
                }
                TxCheckOutcome::Stuck => {
                    // Update the last stuck transaction. If we won't exit the loop early,
                    // it will be used to create a new transaction with higher gas limit.
                    last_stuck_tx = Some(tx);
                }
                TxCheckOutcome::Failed(receipt) => {
                    warn!(
                        "ETH transaction failed: tx: {:#x}, operation_id: {}; tx_receipt: {:#?} ",
                        tx.signed_tx.hash,
                        op.operation.id.unwrap(),
                        receipt,
                    );
                    failed_txs.insert(tx.signed_tx.hash);

                    // TODO: React on a failure. There should be an failed tx processing
                    // policy.
                }
            }
        }

        // Reaching this point will mean that either there was no transactions to process,
        // or the latest transaction got stuck.
        // Either way we should create a new transaction (the approach is the same,
        // `create_new_tx` will adapt its logic based on `last_stuck_tx`).
        let deadline_block = current_block + EXPECTED_WAIT_TIME_BLOCKS;
        let new_tx = self.create_new_tx(&op.operation, deadline_block, last_stuck_tx)?;
        // New transaction should be persisted in the DB *before* sending it.
        self.save_signed_tx_to_db(&new_tx)?;

        op.txs.push(new_tx.clone());
        info!(
            "Sending tx for op, op_id: {} tx_hash: {:#x}",
            new_tx.op_id, new_tx.signed_tx.hash
        );
        self.ethereum.send_tx(&new_tx.signed_tx)?;

        Ok(OperationCommitment::Pending)
    }

    /// Creates a new transaction. If stuck tx is provided, the new transaction will be
    /// and updated version of it; otherwise a brand new transaction will be created.
    fn create_new_tx(
        &self,
        op: &Operation,
        deadline_block: u64,
        stuck_tx: Option<&TransactionETHState>,
    ) -> Result<TransactionETHState, failure::Error> {
        let tx_options = if let Some(stuck_tx) = stuck_tx {
            self.tx_options_from_stuck_tx(stuck_tx)?
        } else {
            Options::default()
        };

        let signed_tx = self.sign_operation_tx(op, tx_options)?;
        Ok(TransactionETHState {
            op_id: op.id.unwrap(),
            deadline_block,
            signed_tx,
        })
    }

    // Calculates a new gas amount for the replacement of the stuck tx.
    // Replacement price should be at least 10% higher, we make it 15% higher.
    fn scale_gas(&self, old_tx_gas_price: U256) -> Result<U256, failure::Error> {
        let network_price = self.ethereum.gas_price()?;
        let replacement_price = (old_tx_gas_price * U256::from(115)) / U256::from(100);
        Ok(std::cmp::max(network_price, replacement_price))
    }

    /// Creates a new tx options from a stuck transaction, with updated gas amount
    /// and nonce.
    fn tx_options_from_stuck_tx(
        &self,
        stuck_tx: &TransactionETHState,
    ) -> Result<Options, failure::Error> {
        let old_tx_gas_price =
            U256::from_dec_str(&stuck_tx.signed_tx.gas_price.to_string()).unwrap();

        let new_gas_price = self.scale_gas(old_tx_gas_price)?;
        let new_nonce = self.ethereum.current_nonce()?;

        info!(
            "Replacing tx: hash: {:#x}, old_gas: {}, new_gas: {}, old_nonce: {}, new_nonce: {}",
            stuck_tx.signed_tx.hash,
            old_tx_gas_price,
            new_gas_price,
            stuck_tx.signed_tx.nonce,
            new_nonce
        );

        Ok(Options::with(move |opt| {
            opt.gas_price = Some(new_gas_price);
            opt.nonce = Some(new_nonce);
        }))
    }

    fn sign_operation_tx(
        &self,
        op: &Operation,
        tx_options: Options,
    ) -> Result<SignedCallResult, failure::Error> {
        match &op.action {
            Action::Commit => {
                //                let mut block_number = op.block.block_number;
                //                // FAIL TEST
                //                let rnd = rand::thread_rng().gen_range(0,10);
                //                if rnd < 5 {
                //                    error!("Messing with tx pubdata");
                //                    block_number += rnd + 1;
                //                }
                //                // FAIL TEST
                let root = op.block.get_eth_encoded_root();

                let public_data = op.block.get_eth_public_data();
                debug!(
                    "public_data for block_number {}: {}",
                    op.block.block_number,
                    hex::encode(&public_data)
                );

                let witness_data = op.block.get_eth_witness_data();
                debug!(
                    "witness_data for block {}: {}, {:?}",
                    op.block.block_number,
                    hex::encode(&witness_data.0),
                    &witness_data.1
                );

                // function commitBlock(uint32 _blockNumber, uint24 _feeAccount, bytes32 _newRoot, bytes calldata _publicData)
                self.ethereum.sign_call_tx(
                    "commitBlock",
                    (
                        u64::from(op.block.block_number),
                        u64::from(op.block.fee_account),
                        root,
                        public_data,
                        witness_data.0,
                        witness_data.1,
                    ),
                    tx_options,
                )
            }
            Action::Verify { proof } => {
                // function verifyBlock(uint32 _blockNumber, uint256[8] calldata proof) external {
                self.ethereum.sign_call_tx(
                    "verifyBlock",
                    (u64::from(op.block.block_number), *proof.clone()),
                    tx_options,
                )
            }
        }
    }

    fn call_complete_withdrawals(&self) -> Result<(), failure::Error> {
        // function completeWithdrawals(uint32 _n) external {
        let tx = self
            .ethereum
            .sign_call_tx(
                "completeWithdrawals",
                config::MAX_WITHDRAWALS_TO_COMPLETE_IN_A_CALL,
                Options::default(),
            )
            .map_err(|e| failure::format_err!("completeWithdrawals: {}", e))?;
        info!("Sending completeWithdrawals tx with hash: {:#?}", tx.hash);
        self.ethereum.send_tx(&tx)
    }
}

pub fn start_eth_sender(
    pool: ConnectionPool,
    panic_notify: mpsc::Sender<bool>,
    op_notify_sender: mpsc::Sender<Operation>,
    send_requst_receiver: mpsc::Receiver<Operation>,
    config_options: ConfigurationOptions,
) {
    std::thread::Builder::new()
        .name("eth_sender".to_string())
        .spawn(move || {
            let _panic_sentinel = ThreadPanicNotify(panic_notify);

            let ethereum =
                EthereumHttpClient::new(&config_options).expect("Ethereum client creation failed");

            let mut runtime = Runtime::new().expect("eth-sender-runtime");
            let eth_sender = ETHSender::new(pool, ethereum, send_requst_receiver, op_notify_sender);
            runtime.block_on(eth_sender.run());
        })
        .expect("Eth sender thread");
}
