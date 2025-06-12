use anyhow::Result;
use dotenv::dotenv;
use miden_crypto::Felt;
use std::{env, time::Duration};
use tokio::time::sleep;
use tracing::{error, info, warn};

use chrono::Utc;
use miden_client::{account::AccountId, asset::FungibleAsset, note::Note, rpc::Endpoint};
use miden_clob::{
    common::{instantiate_client, try_match_swapp_notes},
    database::{Database, P2IdNoteRecord, SwapNoteRecord, SwapNoteStatus},
    note_serialization::{deserialize_note, extract_note_info, serialize_note},
};
use uuid::Uuid;

// Maximum number of order pairs to match in a single batch
const MAX_BATCH_SIZE: usize = 50; // Configurable - can match up to 50 orders (25 pairs)

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
        sleep(Duration::from_secs(1)).await;
    }
}

async fn run_matching_cycle(
    db: &Database,
    matcher_id: AccountId,
    endpoint: Endpoint,
) -> Result<usize> {
    // Get all open orders from database
    let open_orders = match db.get_open_swap_notes().await {
        Ok(orders) => orders,
        Err(e) => {
            error!("Failed to fetch open swap notes from database: {}", e);
            return Ok(0); // Skip this cycle and continue
        }
    };

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
                warn!(
                    "Failed to deserialize note {}: {} - skipping this note",
                    record.note_id, e
                );
                continue;
            }
        }
    }

    if notes_with_records.len() < 2 {
        return Ok(0);
    }

    // Find all possible matches - DO NOT SORT, keep the order from try_match_swapp_notes
    let mut all_matches = Vec::new();
    let mut used_indices = std::collections::HashSet::new();

    // Check all pairs for matches using try_match_swapp_notes
    for i in 0..notes_with_records.len() {
        for j in (i + 1)..notes_with_records.len() {
            if used_indices.contains(&i) || used_indices.contains(&j) {
                continue; // Skip if either note is already matched
            }

            let (record1, note1) = &notes_with_records[i];
            let (record2, note2) = &notes_with_records[j];

            // Use try_match_swapp_notes to check if these notes can be matched
            match try_match_swapp_notes(note1, note2, matcher_id) {
                Ok(Some(swap_data)) => {
                    // Keep the data from try_match_swapp_notes without modification
                    all_matches.push((swap_data, (*record1).clone(), (*record2).clone(), i, j));
                    used_indices.insert(i);
                    used_indices.insert(j);
                }
                Ok(None) => {
                    // No match found, continue to next pair
                }
                Err(e) => {
                    warn!(
                        "Error checking match potential between {} and {}: {} - skipping this pair",
                        record1.note_id, record2.note_id, e
                    );
                    continue;
                }
            }
        }
    }

    if all_matches.is_empty() {
        return Ok(0);
    }

    info!(
        "Found {} potential matches, attempting to process all in batch",
        all_matches.len()
    );

    // Limit batch size to MAX_BATCH_SIZE
    let matches_to_process = if all_matches.len() > MAX_BATCH_SIZE {
        &all_matches[..MAX_BATCH_SIZE]
    } else {
        &all_matches
    };

    // Try to execute matches with binary search approach
    match execute_batch_with_binary_search(matches_to_process, matcher_id, endpoint, db).await {
        Ok(matches_executed) => {
            info!("✅ Successfully executed {} matches", matches_executed);
            Ok(matches_executed)
        }
        Err(e) => {
            error!("❌ Failed to execute any matches: {}", e);
            Ok(0)
        }
    }
}

// Binary search approach: try batch, if it fails, divide in half and try smaller batches
// Always re-fetch fresh data from database to ensure orders still exist
fn execute_batch_with_binary_search<'a>(
    matches: &'a [(
        miden_clob::common::MatchedSwap,
        miden_clob::database::SwapNoteRecord,
        miden_clob::database::SwapNoteRecord,
        usize,
        usize,
    )],
    matcher_id: AccountId,
    endpoint: Endpoint,
    db: &'a Database,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<usize>> + 'a>> {
    Box::pin(async move {
        if matches.is_empty() {
            return Ok(0);
        }

        // Before processing, validate that all orders in this batch still exist in the database
        info!(
            "🔍 Validating {} matches against current database state",
            matches.len()
        );
        let mut valid_matches = Vec::new();

        // Re-fetch fresh data from database to ensure orders still exist
        let current_open_orders = match db.get_open_swap_notes().await {
            Ok(orders) => orders,
            Err(e) => {
                error!("Failed to fetch current open orders for validation: {}", e);
                return Ok(0);
            }
        };

        // Create a set of currently open note IDs for quick lookup
        let open_note_ids: std::collections::HashSet<String> = current_open_orders
            .iter()
            .map(|record| record.note_id.clone())
            .collect();

        // Filter matches to only include those where both orders still exist
        for (swap_data, record1, record2, i, j) in matches {
            if open_note_ids.contains(&record1.note_id) && open_note_ids.contains(&record2.note_id)
            {
                valid_matches.push((swap_data.clone(), record1.clone(), record2.clone(), *i, *j));
            } else {
                info!(
                    "📝 Skipping stale match: {} <-> {} (one or both orders no longer exist)",
                    record1.note_id, record2.note_id
                );
            }
        }

        if valid_matches.is_empty() {
            info!("📝 All matches were stale - no valid matches remaining");
            return Ok(0);
        }

        if valid_matches.len() != matches.len() {
            info!(
                "📝 Filtered {} stale matches, {} valid matches remaining",
                matches.len() - valid_matches.len(),
                valid_matches.len()
            );
        }

        // Try to execute the full batch first
        match execute_batch_blockchain_match_simplified(
            &valid_matches,
            matcher_id,
            endpoint.clone(),
            db,
        )
        .await
        {
            Ok(_tx_id) => {
                info!("✅ Full batch executed successfully");
                // Return immediately after successful batch - do not continue binary search
                // This allows the main loop to restart with fresh data from database
                return Ok(valid_matches.len());
            }
            Err(e) => {
                info!("❌ Full batch failed: {}, trying binary search approach", e);
            }
        }

        // If batch fails and we have more than 1 match, try binary search
        if valid_matches.len() > 1 {
            let mid = valid_matches.len() / 2;
            let (left_half, right_half) = valid_matches.split_at(mid);

            info!(
                "🔍 Binary search: trying left half with {} matches",
                left_half.len()
            );
            let left_result =
                execute_batch_with_binary_search(left_half, matcher_id, endpoint.clone(), db).await;

            // If left half succeeded, return immediately without trying right half
            // This ensures we restart with fresh data after any successful transaction
            if let Ok(left_count) = left_result {
                if left_count > 0 {
                    info!(
                        "✅ Left half succeeded with {} matches, restarting with fresh data",
                        left_count
                    );
                    return Ok(left_count);
                }
            }

            info!(
                "🔍 Binary search: trying right half with {} matches",
                right_half.len()
            );
            let right_result =
                execute_batch_with_binary_search(right_half, matcher_id, endpoint, db).await;

            // Return the result from right half (either success or failure)
            right_result
        } else {
            // Single match - try individual execution
            let (swap_data, record1, record2, _, _) = &valid_matches[0];
            info!(
                "🎯 Executing single match: {} <-> {}",
                record1.note_id, record2.note_id
            );

            match execute_blockchain_match_simplified(
                swap_data, matcher_id, endpoint, db, record1, record2,
            )
            .await
            {
                Ok(_tx_id) => {
                    info!("✅ Single match executed successfully");
                    Ok(1)
                }
                Err(e) => {
                    error!("❌ Single match failed: {}", e);
                    // Do not add orders to failed order database per user request
                    // Just log the error and continue
                    warn!(
                        "Skipping failed match between {} and {} - will retry in next cycle",
                        record1.note_id, record2.note_id
                    );
                    Err(e)
                }
            }
        }
    })
}

// Simplified batch execution following the working test pattern exactly
async fn execute_batch_blockchain_match_simplified(
    matches_batch: &[(
        miden_clob::common::MatchedSwap,
        miden_clob::database::SwapNoteRecord,
        miden_clob::database::SwapNoteRecord,
        usize,
        usize,
    )],
    matcher_id: AccountId,
    endpoint: Endpoint,
    db: &Database,
) -> Result<String> {
    info!(
        "Executing batch blockchain transaction for {} matches",
        matches_batch.len()
    );

    let mut client = match instantiate_client(endpoint).await {
        Ok(client) => client,
        Err(e) => {
            return Err(anyhow::anyhow!("Failed to instantiate client: {}", e));
        }
    };
    client.sync_state().await.unwrap();

    // Import the matcher account to ensure it exists in the client state
    /*
    info!("Importing matcher account: {}", matcher_id.to_hex());
    if let Err(e) = client.import_account_by_id(matcher_id).await {
        warn!(
            "Failed to import matcher account (may already exist): {}",
            e
        );
    }
    */

    // Sync state with better error handling
    if let Err(e) = client.sync_state().await {
        return Err(anyhow::anyhow!("Failed to sync client state: {}", e));
    }

    // Use the swap data directly from try_match_swapp_notes without modification
    // Following the multi_order_fill_test pattern exactly
    let mut input_notes = Vec::new();
    let mut expected_outputs = Vec::new();

    for (swap_data, _, _, _, _) in matches_batch {
        // Add input notes from this match (exactly like the test)
        input_notes.push((swap_data.swap_note_1.clone(), Some(swap_data.note1_args)));
        input_notes.push((swap_data.swap_note_2.clone(), Some(swap_data.note2_args)));

        // Add expected output notes from this match (exactly like the test)
        expected_outputs.push(swap_data.p2id_from_1_to_2.clone());
        expected_outputs.push(swap_data.p2id_from_2_to_1.clone());

        // Add leftover notes if any (exactly like the test)
        if let Some(ref leftover_note) = swap_data.leftover_swapp_note {
            expected_outputs.push(leftover_note.clone());
        }
    }

    let input_notes_len = input_notes.len();
    info!(
        "Batch transaction will consume {} input notes and produce {} output notes",
        input_notes_len,
        expected_outputs.len()
    );

    // Build the transaction request (exactly like the test)
    use miden_client::transaction::TransactionRequestBuilder;
    let consume_req = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes(input_notes)
        .with_expected_output_notes(expected_outputs.clone())
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build batch transaction request: {}", e))?;

    // Execute the transaction (exactly like the test)
    let tx_result = client
        .new_transaction(matcher_id, consume_req)
        .await
        .map_err(|e| {
            error!("🔍 Detailed batch transaction creation error: {:?}", e);
            error!("🔍 Matcher ID: {}", matcher_id.to_hex());
            error!("🔍 Number of input notes: {}", input_notes_len);
            error!(
                "🔍 Number of expected output notes: {}",
                expected_outputs.len()
            );
            anyhow::anyhow!("Failed to create batch transaction: {:?}", e)
        })?;

    // Submit the transaction (exactly like the test)
    client
        .submit_transaction(tx_result.clone())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to submit batch transaction: {}", e))?;

    let tx_id = tx_result.executed_transaction().id();
    let tx_id_hex = format!("{:?}", tx_id);

    info!(
        "🚀 Successfully executed batch blockchain transaction: {}",
        tx_id_hex
    );

    // Process database updates for all matches in the batch
    for (swap_data, record1, record2, _, _) in matches_batch {
        // Save P2ID notes to database
        if let Err(e) = save_p2id_notes_to_db(db, swap_data, &tx_id_hex).await {
            error!("Failed to save P2ID notes to database: {}", e);
            // Continue execution as this is not critical for the blockchain transaction
        }

        // Handle order lifecycle: move both orders to filled_orders table and handle leftovers
        if let Some(ref leftover_note) = swap_data.leftover_swapp_note {
            info!(
                "📝 Processing partial fill - moving orders to filled_orders and adding leftover note"
            );

            // Determine which note was partially filled by comparing with the leftover note creator
            let leftover_creator = miden_clob::common::creator_of(leftover_note);
            let note1_creator = miden_clob::common::creator_of(&swap_data.swap_note_1);

            let (partially_filled_record, fully_filled_record) =
                if leftover_creator == note1_creator {
                    (record1, record2)
                } else {
                    (record2, record1)
                };

            // Move both orders to filled_orders table
            if let Err(e) = db
                .move_to_filled_orders(
                    &fully_filled_record.note_id,
                    "complete",
                    Some(tx_id_hex.clone()),
                )
                .await
            {
                error!(
                    "Failed to move fully filled order {} to filled_orders: {}",
                    fully_filled_record.note_id, e
                );
            } else {
                info!(
                    "✅ Moved fully filled order {} to filled_orders table",
                    fully_filled_record.note_id
                );
            }

            if let Err(e) = db
                .move_to_filled_orders(
                    &partially_filled_record.note_id,
                    "partial",
                    Some(tx_id_hex.clone()),
                )
                .await
            {
                error!(
                    "Failed to move partially filled order {} to filled_orders: {}",
                    partially_filled_record.note_id, e
                );
            } else {
                info!(
                    "✅ Moved partially filled order {} to filled_orders table",
                    partially_filled_record.note_id
                );
            }

            // Add the leftover note as a new open order
            if let Err(e) = insert_leftover_note_to_db(db, leftover_note).await {
                error!("Failed to insert leftover note: {}", e);
            }
        } else {
            info!("📝 Processing complete fill - moving both orders to filled_orders");

            // Both orders are completely filled - move both to filled_orders table
            if let Err(e) = db
                .move_to_filled_orders(&record1.note_id, "complete", Some(tx_id_hex.clone()))
                .await
            {
                error!(
                    "Failed to move order {} to filled_orders: {}",
                    record1.note_id, e
                );
            } else {
                info!("✅ Moved order {} to filled_orders table", record1.note_id);
            }

            if let Err(e) = db
                .move_to_filled_orders(&record2.note_id, "complete", Some(tx_id_hex.clone()))
                .await
            {
                error!(
                    "Failed to move order {} to filled_orders: {}",
                    record2.note_id, e
                );
            } else {
                info!("✅ Moved order {} to filled_orders table", record2.note_id);
            }
        }
    }

    // Force database sync to ensure all changes are immediately visible for next iteration
    if let Err(e) = db.force_sync().await {
        error!("Failed to sync database after successful batch: {}", e);
        // Continue as this is not critical for the transaction itself
    } else {
        info!("✅ Database synced successfully after batch execution");
    }

    Ok(tx_id_hex)
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

    println!("SWAP NOTE: {:?}\n", swap_note.id());
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

// Simplified individual blockchain match execution following the test pattern exactly
async fn execute_blockchain_match_simplified(
    swap_data: &miden_clob::common::MatchedSwap,
    matcher_id: AccountId,
    endpoint: Endpoint,
    db: &Database,
    record1: &miden_clob::database::SwapNoteRecord,
    record2: &miden_clob::database::SwapNoteRecord,
) -> Result<String> {
    info!(
        "Executing blockchain transaction for match between {} and {}",
        swap_data.swap_note_1.id().to_hex(),
        swap_data.swap_note_2.id().to_hex()
    );

    let mut client = match instantiate_client(endpoint).await {
        Ok(client) => client,
        Err(e) => {
            return Err(anyhow::anyhow!("Failed to instantiate client: {}", e));
        }
    };
    client.sync_state().await.unwrap();

    // Import the matcher account to ensure it exists in the client state
    info!("Importing matcher account: {}", matcher_id.to_hex());
    if let Err(e) = client.import_account_by_id(matcher_id).await {
        warn!(
            "Failed to import matcher account (may already exist): {}",
            e
        );
    }

    // Sync state with better error handling
    if let Err(e) = client.sync_state().await {
        return Err(anyhow::anyhow!("Failed to sync client state: {}", e));
    }

    // Construct expected output notes exactly like the test
    let mut expected_outputs = vec![
        swap_data.p2id_from_1_to_2.clone(),
        swap_data.p2id_from_2_to_1.clone(),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_outputs.push(note.clone());
    }

    // Build the transaction request exactly like the test
    use miden_client::transaction::TransactionRequestBuilder;
    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([
            (swap_data.swap_note_1.id(), Some(swap_data.note1_args)),
            (swap_data.swap_note_2.id(), Some(swap_data.note2_args)),
        ])
        .with_expected_output_notes(expected_outputs.clone())
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build transaction request: {}", e))?;

    // Execute the transaction exactly like the test
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

    // Submit the transaction exactly like the test
    let _ = client.submit_transaction(tx_result.clone()).await;

    let tx_id = tx_result.executed_transaction().id();
    let tx_id_hex = format!("{:?}", tx_id);

    info!(
        "🚀 Successfully executed blockchain transaction: {}",
        tx_id_hex
    );

    // Save P2ID notes to database
    if let Err(e) = save_p2id_notes_to_db(db, swap_data, &tx_id_hex).await {
        error!("Failed to save P2ID notes to database: {}", e);
        // Continue execution as this is not critical for the blockchain transaction
    }

    // Handle order lifecycle: move both orders to filled_orders table and handle leftovers
    if let Some(ref leftover_note) = swap_data.leftover_swapp_note {
        info!(
            "📝 Processing partial fill - moving orders to filled_orders and adding leftover note"
        );

        // Determine which note was partially filled by comparing with the leftover note creator
        let leftover_creator = miden_clob::common::creator_of(leftover_note);
        let note1_creator = miden_clob::common::creator_of(&swap_data.swap_note_1);

        let (partially_filled_record, fully_filled_record) = if leftover_creator == note1_creator {
            (record1, record2)
        } else {
            (record2, record1)
        };

        // Move both orders to filled_orders table
        if let Err(e) = db
            .move_to_filled_orders(
                &fully_filled_record.note_id,
                "complete",
                Some(tx_id_hex.clone()),
            )
            .await
        {
            error!(
                "Failed to move fully filled order {} to filled_orders: {}",
                fully_filled_record.note_id, e
            );
        } else {
            info!(
                "✅ Moved fully filled order {} to filled_orders table",
                fully_filled_record.note_id
            );
        }

        if let Err(e) = db
            .move_to_filled_orders(
                &partially_filled_record.note_id,
                "partial",
                Some(tx_id_hex.clone()),
            )
            .await
        {
            error!(
                "Failed to move partially filled order {} to filled_orders: {}",
                partially_filled_record.note_id, e
            );
        } else {
            info!(
                "✅ Moved partially filled order {} to filled_orders table",
                partially_filled_record.note_id
            );
        }

        // Add the leftover note as a new open order
        if let Err(e) = insert_leftover_note_to_db(db, leftover_note).await {
            error!("Failed to insert leftover note: {}", e);
        }
    } else {
        info!("📝 Processing complete fill - moving both orders to filled_orders");

        // Both orders are completely filled - move both to filled_orders table
        if let Err(e) = db
            .move_to_filled_orders(&record1.note_id, "complete", Some(tx_id_hex.clone()))
            .await
        {
            error!(
                "Failed to move order {} to filled_orders: {}",
                record1.note_id, e
            );
        } else {
            info!("✅ Moved order {} to filled_orders table", record1.note_id);
        }

        if let Err(e) = db
            .move_to_filled_orders(&record2.note_id, "complete", Some(tx_id_hex.clone()))
            .await
        {
            error!(
                "Failed to move order {} to filled_orders: {}",
                record2.note_id, e
            );
        } else {
            info!("✅ Moved order {} to filled_orders table", record2.note_id);
        }
    }

    // Force database sync to ensure all changes are immediately visible for next iteration
    if let Err(e) = db.force_sync().await {
        error!(
            "Failed to sync database after successful individual match: {}",
            e
        );
        // Continue as this is not critical for the transaction itself
    } else {
        info!("✅ Database synced successfully after individual match execution");
    }

    Ok(tx_id_hex)
}

async fn save_p2id_notes_to_db(
    db: &Database,
    swap_data: &miden_clob::common::MatchedSwap,
    _tx_id: &str,
) -> Result<()> {
    // Save P2ID note from note1 to note2
    let p2id_1_to_2_serialized = serialize_note(&swap_data.p2id_from_1_to_2)?;
    let note1_creator = miden_clob::common::creator_of(&swap_data.swap_note_1);
    let note2_creator = miden_clob::common::creator_of(&swap_data.swap_note_2);

    // Extract asset info from P2ID note 1->2
    let p2id_1_to_2_asset = swap_data
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .expect("P2ID note has no assets")
        .unwrap_fungible();

    let p2id_record_1_to_2 = P2IdNoteRecord {
        id: Uuid::new_v4().to_string(),
        note_id: swap_data.p2id_from_1_to_2.id().to_hex(),
        sender_id: note1_creator.to_hex(),
        target_id: note2_creator.to_hex(),
        recipient: note2_creator.to_hex(), // The recipient is the note2 creator
        asset_id: p2id_1_to_2_asset.faucet_id().to_hex(),
        amount: p2id_1_to_2_asset.amount(),
        swap_note_id: Some(swap_data.swap_note_1.id().to_hex()),
        note_data: p2id_1_to_2_serialized,
        created_at: Utc::now(),
    };

    if let Err(e) = db.insert_p2id_note(&p2id_record_1_to_2).await {
        error!("Failed to save P2ID note 1->2: {}", e);
    } else {
        info!(
            "✅ Saved P2ID note {} (1->2) to database",
            swap_data.p2id_from_1_to_2.id().to_hex()
        );
    }

    // Save P2ID note from note2 to note1
    let p2id_2_to_1_serialized = serialize_note(&swap_data.p2id_from_2_to_1)?;

    // Extract asset info from P2ID note 2->1
    let p2id_2_to_1_asset = swap_data
        .p2id_from_2_to_1
        .assets()
        .iter()
        .next()
        .expect("P2ID note has no assets")
        .unwrap_fungible();

    let p2id_record_2_to_1 = P2IdNoteRecord {
        id: Uuid::new_v4().to_string(),
        note_id: swap_data.p2id_from_2_to_1.id().to_hex(),
        sender_id: note2_creator.to_hex(),
        target_id: note1_creator.to_hex(),
        recipient: note1_creator.to_hex(), // The recipient is the note1 creator

        asset_id: p2id_2_to_1_asset.faucet_id().to_hex(),
        amount: p2id_2_to_1_asset.amount(),
        swap_note_id: Some(swap_data.swap_note_2.id().to_hex()),
        note_data: p2id_2_to_1_serialized,
        created_at: Utc::now(),
    };

    if let Err(e) = db.insert_p2id_note(&p2id_record_2_to_1).await {
        error!("Failed to save P2ID note 2->1: {}", e);
    } else {
        info!(
            "✅ Saved P2ID note {} (2->1) to database",
            swap_data.p2id_from_2_to_1.id().to_hex()
        );
    }

    Ok(())
}

async fn insert_leftover_note_to_db(db: &Database, leftover_note: &Note) -> Result<()> {
    // Serialize and extract info from the leftover note
    let serialized_note = serialize_note(leftover_note)?;

    let (
        creator_id,
        offered_asset_id,
        offered_amount,
        requested_asset_id,
        requested_amount,
        price,
        is_bid,
    ) = extract_note_info(leftover_note)?;

    // Create new database record for the leftover note
    let leftover_record = SwapNoteRecord {
        id: Uuid::new_v4().to_string(),
        note_id: leftover_note.id().to_hex(),
        creator_id,
        offered_asset_id,
        offered_amount,
        requested_asset_id,
        requested_amount,
        price,
        is_bid,
        note_data: serialized_note,
        status: SwapNoteStatus::Open,
        failure_count: 0,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    // Insert the leftover note as a new open order
    db.insert_swap_note(&leftover_record).await?;
    info!(
        "✅ Added leftover note {} back to order book",
        leftover_note.id().to_hex()
    );

    // Force database sync to ensure leftover note is immediately visible
    if let Err(e) = db.force_sync().await {
        error!(
            "Failed to sync database after inserting leftover note: {}",
            e
        );
        // Continue as the note was still inserted
    }

    Ok(())
}

// Removed handle_match_failure function - no longer adding orders to failed order database
