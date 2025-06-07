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
const MAX_BATCH_SIZE: usize = 50; // Configurable - can match up to 100 orders (50 pairs)

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

    // Find all possible matches and sort them by price priority
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
                    // Calculate price for sorting (highest price buy orders first)
                    let price_priority = calculate_match_priority(note1, note2);

                    all_matches.push((
                        price_priority,
                        swap_data,
                        (*record1).clone(),
                        (*record2).clone(),
                        i,
                        j,
                    ));
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

    // Sort matches by price priority (highest first)
    all_matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Limit batch size
    let batch_size = std::cmp::min(all_matches.len(), MAX_BATCH_SIZE);
    let matches_batch = &all_matches[..batch_size];

    info!(
        "Found {} potential matches, processing batch of {}",
        all_matches.len(),
        batch_size
    );

    // Execute batch transaction if we have matches
    if !matches_batch.is_empty() {
        match execute_batch_blockchain_match(matches_batch, matcher_id, endpoint.clone(), db).await
        {
            Ok(tx_id) => {
                info!("✅ Batch blockchain transaction executed: {}", tx_id);
                info!("🎯 {} matches completed successfully in batch", batch_size);
                return Ok(batch_size);
            }
            Err(e) => {
                error!(
                    "❌ Failed to execute batch blockchain transaction: {} - falling back to individual matches",
                    e
                );

                // Fall back to individual matching if batch fails
                return execute_individual_matches(&all_matches[0..1], matcher_id, endpoint, db)
                    .await;
            }
        }
    }

    Ok(0)
}

// Calculate priority for match ordering (highest buy price first, lowest sell price first)
fn calculate_match_priority(note1: &Note, note2: &Note) -> f64 {
    let (offer1, want1) = match miden_clob::common::decompose_swapp_note(note1) {
        Ok((o, w)) => (o, w),
        Err(_) => return 0.0,
    };
    let (offer2, want2) = match miden_clob::common::decompose_swapp_note(note2) {
        Ok((o, w)) => (o, w),
        Err(_) => return 0.0,
    };

    // Calculate effective prices for both sides
    let price1 = if want1.amount() > 0 {
        offer1.amount() as f64 / want1.amount() as f64
    } else {
        0.0
    };

    let price2 = if want2.amount() > 0 {
        offer2.amount() as f64 / want2.amount() as f64
    } else {
        0.0
    };

    // Return higher price as priority (for better price discovery)
    f64::max(price1, price2)
}

// Execute individual matches as fallback
async fn execute_individual_matches(
    matches: &[(
        f64,
        miden_clob::common::MatchedSwap,
        miden_clob::database::SwapNoteRecord,
        miden_clob::database::SwapNoteRecord,
        usize,
        usize,
    )],
    matcher_id: AccountId,
    endpoint: Endpoint,
    db: &Database,
) -> Result<usize> {
    let mut matches_executed = 0;

    for (_, swap_data, record1, record2, _, _) in matches {
        info!("Found match: {} <-> {}", record1.note_id, record2.note_id);

        print_swap_note_data(swap_data.swap_note_1.clone());
        print_swap_note_data(swap_data.swap_note_2.clone());
        println!("\nswap data: {:?} \n", swap_data);

        // Execute the blockchain transaction with enhanced error handling
        match execute_blockchain_match(
            swap_data,
            matcher_id,
            endpoint.clone(),
            db,
            record1,
            record2,
        )
        .await
        {
            Ok(tx_id) => {
                info!("✅ Blockchain transaction executed: {}", tx_id);
                matches_executed += 1;
                info!("🎯 Match #{} completed successfully", matches_executed);

                // For now, only process one match per cycle to avoid conflicts
                return Ok(matches_executed);
            }
            Err(e) => {
                error!(
                    "❌ Failed to execute blockchain transaction for match {} <-> {}: {} - tracking failure and skipping to next potential match",
                    record1.note_id, record2.note_id, e
                );

                // Track failures for both notes involved in the failed match
                if let Err(db_err) =
                    handle_match_failure(db, &record1.note_id, &e.to_string()).await
                {
                    error!(
                        "Failed to track failure for note {}: {}",
                        record1.note_id, db_err
                    );
                }
                if let Err(db_err) =
                    handle_match_failure(db, &record2.note_id, &e.to_string()).await
                {
                    error!(
                        "Failed to track failure for note {}: {}",
                        record2.note_id, db_err
                    );
                }

                // Continue to next potential match instead of failing the entire cycle
                continue;
            }
        }
    }

    Ok(matches_executed)
}

async fn execute_batch_blockchain_match(
    matches_batch: &[(
        f64,
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

    // Re-compute matches fresh to ensure we have current state
    let mut fresh_swap_data = Vec::new();
    let mut all_records = Vec::new();

    for (_, _, record1, record2, _, _) in matches_batch {
        // Deserialize the notes fresh from database
        let note1 = deserialize_note(&record1.note_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize note1: {}", e))?;
        let note2 = deserialize_note(&record2.note_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize note2: {}", e))?;

        // Re-compute the match data fresh
        let swap_data = try_match_swapp_notes(&note1, &note2, matcher_id)
            .map_err(|e| anyhow::anyhow!("Failed to match notes: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("Notes no longer match"))?;

        fresh_swap_data.push(swap_data);
        all_records.push((record1.clone(), record2.clone()));
    }

    // Collect all input notes and expected outputs using fresh data
    let mut input_notes = Vec::new();
    let mut expected_outputs = Vec::new();

    for swap_data in &fresh_swap_data {
        // Add input notes from this match
        input_notes.push((swap_data.swap_note_1.id(), Some(swap_data.note1_args)));
        input_notes.push((swap_data.swap_note_2.id(), Some(swap_data.note2_args)));

        // Add expected output notes from this match
        expected_outputs.push(swap_data.p2id_from_1_to_2.clone());
        expected_outputs.push(swap_data.p2id_from_2_to_1.clone());

        // Add leftover notes if any
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

    // Build the transaction request
    use miden_client::transaction::TransactionRequestBuilder;
    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes(input_notes)
        .with_expected_output_notes(expected_outputs.clone())
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build batch transaction request: {}", e))?;

    // Execute the transaction
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

    // Submit the transaction
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
    for (swap_data, (record1, record2)) in fresh_swap_data.iter().zip(all_records.iter()) {
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

async fn execute_blockchain_match(
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

    // Construct expected output notes (P2ID notes + leftover note if any)
    let mut expected_outputs = vec![
        swap_data.p2id_from_1_to_2.clone(),
        swap_data.p2id_from_2_to_1.clone(),
    ];
    if let Some(ref leftover_note) = swap_data.leftover_swapp_note {
        expected_outputs.push(leftover_note.clone());
    }

    // Build the transaction request
    use miden_client::transaction::TransactionRequestBuilder;
    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([
            (swap_data.swap_note_1.id(), Some(swap_data.note1_args)),
            (swap_data.swap_note_2.id(), Some(swap_data.note2_args)),
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

    Ok(tx_id_hex)
}

async fn save_p2id_notes_to_db(
    db: &Database,
    swap_data: &miden_clob::common::MatchedSwap,
    tx_id: &str,
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

    Ok(())
}

async fn handle_match_failure(db: &Database, note_id: &str, failure_reason: &str) -> Result<()> {
    // Increment the failure count for this note
    match db.increment_failure_count(note_id).await {
        Ok(failure_count) => {
            info!("📊 Note {} failure count: {}", note_id, failure_count);

            // If failure count exceeds 3, move to failed table
            if failure_count > 3 {
                warn!(
                    "🚫 Note {} has failed {} times, moving to failed orders table",
                    note_id, failure_count
                );

                match db.move_to_failed_table(note_id, failure_reason).await {
                    Ok(_) => {
                        info!(
                            "✅ Successfully moved note {} to failed orders table",
                            note_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "❌ Failed to move note {} to failed orders table: {}",
                            note_id, e
                        );
                        return Err(e);
                    }
                }
            }
        }
        Err(e) => {
            error!(
                "❌ Failed to increment failure count for note {}: {}",
                note_id, e
            );
            return Err(e);
        }
    }

    Ok(())
}
