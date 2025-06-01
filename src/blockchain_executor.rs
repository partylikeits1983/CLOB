use anyhow::Result;
use tracing::info;

use miden_client::{
    account::AccountId, note::Note, rpc::Endpoint, transaction::TransactionRequestBuilder,
};

use crate::common::{MatchedSwap, instantiate_client};

/// Execute a blockchain transaction for matched SWAP notes
/// This function is NOT thread-safe and should be called outside of web server context
pub async fn execute_match_transaction(
    note1: &Note,
    note2: &Note,
    swap_data: &MatchedSwap,
    matcher_id: AccountId,
    endpoint: Endpoint,
) -> Result<String> {
    info!(
        "Executing blockchain transaction for match between {} and {}",
        note1.id().to_hex(),
        note2.id().to_hex()
    );

    // Initialize Miden client for blockchain transaction execution
    let mut client = instantiate_client(endpoint).await?;

    // Construct expected output notes (P2ID notes + leftover note if any)
    let mut expected_outputs = vec![
        swap_data.p2id_from_1_to_2.clone(),
        swap_data.p2id_from_2_to_1.clone(),
    ];
    if let Some(ref leftover_note) = swap_data.leftover_swapp_note {
        expected_outputs.push(leftover_note.clone());
    }
    expected_outputs.sort_by_key(|n| n.commitment());

    // Build the transaction request following the matching algorithm test pattern
    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([
            (note1.id(), Some(swap_data.note1_args)),
            (note2.id(), Some(swap_data.note2_args)),
        ])
        .with_expected_output_notes(expected_outputs)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build transaction request: {}", e))?;

    // Execute the transaction on the blockchain
    let tx_result = client
        .new_transaction(matcher_id, consume_req)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create transaction: {}", e))?;

    // Submit the transaction to the blockchain
    client
        .submit_transaction(tx_result.clone())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to submit transaction: {}", e))?;

    let tx_id = tx_result.executed_transaction().id();
    let tx_id_hex = format!("{:?}", tx_id);

    info!(
        "🚀 Successfully executed blockchain transaction for match: {}",
        tx_id_hex
    );
    info!(
        "🔗 View transaction on MidenScan: https://testnet.midenscan.com/tx/{}",
        tx_id_hex
    );

    Ok(tx_id_hex)
}
