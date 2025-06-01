use anyhow::Result;
use dotenv::dotenv;
use miden_crypto::Felt;
use std::{env, time::Duration};
use tokio::time::sleep;
use tracing::{error, info, warn};

use miden_client::{account::AccountId, asset::FungibleAsset, note::Note, rpc::Endpoint};
use miden_clob::{
    common::{instantiate_client, try_match_swapp_notes},
    database::{Database, SwapNoteStatus},
    note_serialization::deserialize_note,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("info,miden_clob=debug")
        .init();

    info!("Starting Miden CLOB Matching Engine");

    // Load environment variables
    dotenv().ok();

    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:./clob.sqlite3".to_string());
    let matcher_account_id = env::var("MATCHER_ACCOUNT_ID")
        .expect("MATCHER_ACCOUNT_ID not found in .env file. Run populate with --setup first.");
    let miden_endpoint =
        env::var("MIDEN_NODE_ENDPOINT").expect("MIDEN_NODE_ENDPOINT not found in .env file.");

    let matcher_id = AccountId::from_hex(&matcher_account_id)
        .map_err(|e| anyhow::anyhow!("Invalid MATCHER_ACCOUNT_ID: {}", e))?;

    let endpoint = Endpoint::try_from(miden_endpoint.as_str())
        .map_err(|e| anyhow::anyhow!("Invalid MIDEN_NODE_ENDPOINT: {}", e))?;

    // Initialize database (will create if it doesn't exist)
    info!("Initializing database: {}", database_url);

    // Ensure the database file exists by creating parent directories if needed
    if let Some(path) = std::path::Path::new(&database_url.replace("sqlite:", "")).parent() {
        std::fs::create_dir_all(path)?;
    }

    let db = Database::new(&database_url).await?;
    info!("Database connected successfully");

    info!("Matching engine initialized");
    info!("Matcher account: {}", matcher_account_id);
    info!("Database: {}", database_url);
    info!("Miden endpoint: {}", miden_endpoint);

    // Run matching loop
    loop {
        match run_matching_cycle(&db, matcher_id, endpoint.clone()).await {
            Ok(matches_found) => {
                if matches_found > 0 {
                    info!(
                        "Matching cycle completed: {} matches executed",
                        matches_found
                    );
                } else {
                    info!("Matching cycle completed: no matches found");
                }
            }
            Err(e) => {
                error!("Error in matching cycle: {}", e);
            }
        }

        // Wait 1 second before next cycle
        sleep(Duration::from_secs(5)).await;
    }
}

async fn run_matching_cycle(
    db: &Database,
    matcher_id: AccountId,
    endpoint: Endpoint,
) -> Result<usize> {
    // Get all open orders from database
    let open_orders = db.get_open_swap_notes().await?;

    if open_orders.len() < 2 {
        return Ok(0);
    }

    // Deserialize notes from database
    let mut notes_with_records = Vec::new();
    for record in &open_orders {
        match deserialize_note(&record.note_data) {
            Ok(note) => {
                notes_with_records.push((record, note));
            }
            Err(e) => {
                warn!("Failed to deserialize note {}: {}", record.note_id, e);
                continue;
            }
        }
    }

    if notes_with_records.len() < 2 {
        return Ok(0);
    }

    let mut matches_executed = 0;

    // Check all pairs for matches using try_match_swapp_notes
    for i in 0..notes_with_records.len() {
        for j in (i + 1)..notes_with_records.len() {
            let (record1, note1) = &notes_with_records[i];
            let (record2, note2) = &notes_with_records[j];

            // Use try_match_swapp_notes to check if these notes can be matched
            match try_match_swapp_notes(note1, note2, matcher_id) {
                Ok(Some(swap_data)) => {
                    info!("Found match: {} <-> {}", record1.note_id, record2.note_id);

                    print_swap_note_data(note1.clone());
                    print_swap_note_data(note2.clone());
                    println!("\nswap data: {:?} \n", swap_data);

                    // Execute the blockchain transaction
                    match execute_blockchain_match(
                        note1,
                        note2,
                        &swap_data,
                        matcher_id,
                        endpoint.clone(),
                    )
                    .await
                    {
                        Ok(tx_id) => {
                            info!("✅ Blockchain transaction executed: {}", tx_id);

                            // Update database to mark orders as filled
                            if let Err(e) = db
                                .update_swap_note_status(&record1.note_id, SwapNoteStatus::Filled)
                                .await
                            {
                                error!("Failed to update order1 status: {}", e);
                            }
                            if let Err(e) = db
                                .update_swap_note_status(&record2.note_id, SwapNoteStatus::Filled)
                                .await
                            {
                                error!("Failed to update order2 status: {}", e);
                            }

                            matches_executed += 1;
                            info!("🎯 Match #{} completed successfully", matches_executed);

                            // For now, only process one match per cycle to avoid conflicts
                            return Ok(matches_executed);
                        }
                        Err(e) => {
                            error!("❌ Failed to execute blockchain transaction: {}", e);
                            continue;
                        }
                    }
                }
                Ok(None) => {
                    // No match found, continue
                }
                Err(e) => {
                    warn!("Error checking match potential: {}", e);
                    continue;
                }
            }
        }
    }

    Ok(matches_executed)
}

fn print_swap_note_data(swap_note: Note) {
    let offered_asset = swap_note
        .assets()
        .iter()
        .next()
        .expect("note has no assets")
        .unwrap_fungible();

    let note_inputs: &[Felt] = swap_note.inputs().values();
    let requested: &[Felt] = note_inputs.get(..4).expect("note has fewer than 4 inputs");

    let requested_id = AccountId::try_from([requested[3], requested[2]]).unwrap();
    let requested_asset = FungibleAsset::new(requested_id, requested[0].as_int()).unwrap();

    println!("SWAP NOTE: {:?}", swap_note.id());
    println!(
        "offered asset {:?} {:?}",
        offered_asset.faucet_id().to_hex(),
        offered_asset.amount()
    );
    println!(
        "requested asset {:?} {:?}",
        requested_asset.faucet_id().to_hex(),
        requested_asset.amount()
    );
}

async fn execute_blockchain_match(
    note1: &miden_client::note::Note,
    note2: &miden_client::note::Note,
    swap_data: &miden_clob::common::MatchedSwap,
    matcher_id: AccountId,
    endpoint: Endpoint,
) -> Result<String> {
    info!(
        "Executing blockchain transaction for match between {} and {}",
        note1.id().to_hex(),
        note2.id().to_hex()
    );

    let mut client = instantiate_client(endpoint).await?;

    // Import the matcher account to ensure it exists in the client state
    info!("Importing matcher account: {}", matcher_id.to_hex());
    if let Err(e) = client.import_account_by_id(matcher_id).await {
        warn!(
            "Failed to import matcher account (may already exist): {}",
            e
        );
    }

    client.sync_state().await.unwrap();

    // Construct expected output notes (P2ID notes + leftover note if any)
    let mut expected_outputs = vec![
        swap_data.p2id_from_1_to_2.clone(),
        swap_data.p2id_from_2_to_1.clone(),
    ];
    if let Some(ref leftover_note) = swap_data.leftover_swapp_note {
        expected_outputs.push(leftover_note.clone());
    }
    expected_outputs.sort_by_key(|n| n.commitment());

    // Build the transaction request
    use miden_client::transaction::TransactionRequestBuilder;
    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([
            (note1.id(), Some(swap_data.note1_args)),
            (note2.id(), Some(swap_data.note2_args)),
        ])
        .with_expected_output_notes(expected_outputs.clone())
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build transaction request: {}", e))?;

    // Execute the transaction
    let tx_result = client
        .new_transaction(matcher_id, consume_req)
        .await
        .map_err(|e| {
            error!("🔍 Detailed transaction creation error: {:?}", e);
            error!("🔍 Matcher ID: {}", matcher_id.to_hex());
            error!("🔍 Number of input notes: {}", 2);
            error!(
                "🔍 Number of expected output notes: {}",
                expected_outputs.len()
            );
            anyhow::anyhow!("Failed to create transaction: {:?}", e)
        })?;

    // Submit the transaction
    client
        .submit_transaction(tx_result.clone())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to submit transaction: {}", e))?;

    let tx_id = tx_result.executed_transaction().id();
    let tx_id_hex = format!("{:?}", tx_id);

    info!(
        "🚀 Successfully executed blockchain transaction: {}",
        tx_id_hex
    );

    Ok(tx_id_hex)
}
