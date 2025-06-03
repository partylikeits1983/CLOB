use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use miden_client::{
    account::AccountId, note::Note, rpc::Endpoint, transaction::TransactionRequestBuilder,
};

use crate::{
    common::{MatchedSwap, instantiate_client, try_match_swapp_notes},
    database::{Database, P2IdNoteRecord, SwapNoteRecord, SwapNoteStatus},
    note_serialization::{deserialize_note, extract_note_info, serialize_note},
};

/// Execute a blockchain transaction for matched SWAP notes
/// This function is NOT thread-safe and should be called outside of web server context
async fn execute_match_transaction(
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

pub struct OrderBookManager {
    // In-memory cache of open orders for fast matching
    open_orders: HashMap<String, Note>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MatchResult {
    pub order1_id: String,
    pub order2_id: String,
    pub executed_price: f64,
    pub executed_quantity: f64,
    pub p2id_notes: Vec<String>,
    pub leftover_order: Option<String>,
}

impl OrderBookManager {
    pub fn new() -> Self {
        Self {
            open_orders: HashMap::new(),
        }
    }

    pub async fn initialize_from_database(&mut self, _db: &Database) -> Result<()> {
        info!("Initializing orderbook from database");
        // For now, we'll skip loading the actual Note objects since deserialization
        // is not yet implemented. In a full implementation, you would deserialize
        // each note and add it to the open_orders HashMap.
        Ok(())
    }

    pub async fn add_swap_note(&mut self, note: Note, db: &Database) -> Result<String> {
        let note_id = note.id().to_hex();
        info!("Adding swap note to orderbook: {}", note_id);

        // Extract information from the note
        let (
            creator_id,
            offered_asset_id,
            offered_amount,
            requested_asset_id,
            requested_amount,
            price,
            is_bid,
        ) = extract_note_info(&note)?;

        // Serialize the note for database storage
        let note_data = serialize_note(&note)?;

        // Create database record
        let record = SwapNoteRecord {
            id: Uuid::new_v4().to_string(),
            note_id: note_id.clone(),
            creator_id,
            offered_asset_id,
            offered_amount,
            requested_asset_id,
            requested_amount,
            price,
            is_bid,
            note_data,
            status: SwapNoteStatus::Open,
            failure_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        // Store in database
        db.insert_swap_note(&record).await?;

        // Add to in-memory cache
        self.open_orders.insert(note_id.clone(), note);

        info!("Successfully added swap note: {}", note_id);
        Ok(record.id)
    }

    pub async fn run_matching_cycle(&mut self, db: &Database) -> Result<Vec<MatchResult>> {
        info!("Starting matching cycle");
        let mut matches = Vec::new();

        // Get current open orders from database
        let open_order_records = db.get_open_swap_notes().await?;

        if open_order_records.len() < 2 {
            info!(
                "Not enough orders for matching (found {})",
                open_order_records.len()
            );
            return Ok(matches);
        }

        // Get the actual Note objects from cache or deserialize them
        let mut open_notes = Vec::new();
        for record in &open_order_records {
            if let Some(note) = self.open_orders.get(&record.note_id) {
                open_notes.push((record, note.clone()));
            } else {
                // Try to deserialize from database
                match deserialize_note(&record.note_data) {
                    Ok(note) => {
                        self.open_orders
                            .insert(record.note_id.clone(), note.clone());
                        open_notes.push((record, note));
                    }
                    Err(e) => {
                        warn!("Failed to deserialize note {}: {}", record.note_id, e);
                        continue;
                    }
                }
            }
        }

        if open_notes.len() < 2 {
            info!(
                "Not enough valid notes for matching (found {})",
                open_notes.len()
            );
            return Ok(matches);
        }

        // Load matcher account ID from environment
        use dotenv::dotenv;
        use std::env;

        dotenv().ok();
        let matcher_account_id = env::var("MATCHER_ACCOUNT_ID").map_err(|_| {
            anyhow::anyhow!(
                "MATCHER_ACCOUNT_ID not found in .env file. Run populate with --setup first."
            )
        })?;

        let matcher_id = AccountId::from_hex(&matcher_account_id)
            .map_err(|e| anyhow::anyhow!("Invalid MATCHER_ACCOUNT_ID in .env file: {}", e))?;

        // Find crossing orders using the real matching algorithm
        let mut matched_indices = Vec::new();
        for i in 0..open_notes.len() {
            // Skip if this order was already matched in a previous iteration
            if matched_indices.iter().any(|(a, b)| *a == i || *b == i) {
                continue;
            }

            for j in (i + 1)..open_notes.len() {
                // Skip if this order was already matched
                if matched_indices.iter().any(|(a, b)| *a == j || *b == j) {
                    continue;
                }

                let (record1, note1) = &open_notes[i];
                let (record2, note2) = &open_notes[j];

                // Use the real matching algorithm
                if let Ok(Some(swap_data)) = try_match_swapp_notes(note1, note2, matcher_id) {
                    info!(
                        "Found matching orders: {} <-> {}",
                        record1.note_id, record2.note_id
                    );

                    // Immediately mark orders as being processed to prevent race conditions
                    if let Err(e) = db
                        .update_swap_note_status(&record1.note_id, SwapNoteStatus::PartiallyFilled)
                        .await
                    {
                        warn!("Failed to update order1 status during match setup: {}", e);
                        continue;
                    }
                    if let Err(e) = db
                        .update_swap_note_status(&record2.note_id, SwapNoteStatus::PartiallyFilled)
                        .await
                    {
                        warn!("Failed to update order2 status during match setup: {}", e);
                        continue;
                    }

                    match self
                        .execute_real_match(record1, record2, &swap_data, db)
                        .await
                    {
                        Ok(match_result) => {
                            matches.push(match_result);
                            matched_indices.push((i, j));
                        }
                        Err(e) => {
                            error!(
                                "Failed to execute match between {} and {}: {}",
                                record1.note_id, record2.note_id, e
                            );
                            // Revert the status changes if the match failed
                            let _ = db
                                .update_swap_note_status(&record1.note_id, SwapNoteStatus::Open)
                                .await;
                            let _ = db
                                .update_swap_note_status(&record2.note_id, SwapNoteStatus::Open)
                                .await;
                        }
                    }
                    break; // Only match one pair per order for simplicity
                }
            }
        }

        // Remove matched notes from cache
        for (i, j) in matched_indices {
            let (record1, _) = &open_notes[i];
            let (record2, _) = &open_notes[j];
            self.open_orders.remove(&record1.note_id);
            self.open_orders.remove(&record2.note_id);
        }

        info!(
            "Matching cycle completed. Executed {} matches",
            matches.len()
        );
        Ok(matches)
    }

    async fn execute_real_match(
        &mut self,
        order1: &SwapNoteRecord,
        order2: &SwapNoteRecord,
        swap_data: &crate::common::MatchedSwap,
        db: &Database,
    ) -> Result<MatchResult> {
        use dotenv::dotenv;
        use std::env;

        info!(
            "Executing real match between {} and {}",
            order1.note_id, order2.note_id
        );

        /* ---------- 0. Environment ---------- */
        dotenv().ok();
        let matcher_account_id = env::var("MATCHER_ACCOUNT_ID").map_err(|_| {
            anyhow::anyhow!(
                "MATCHER_ACCOUNT_ID not found in .env file. Run populate with --setup first."
            )
        })?;

        let matcher_id = AccountId::from_hex(&matcher_account_id)
            .map_err(|e| anyhow::anyhow!("Invalid MATCHER_ACCOUNT_ID in .env file: {}", e))?;

        /* ---------- 1. Race-condition guard ---------- */
        let p2id1_note_id = swap_data.p2id_from_1_to_2.id().to_hex();
        let p2id2_note_id = swap_data.p2id_from_2_to_1.id().to_hex();

        if let Ok(existing) = db
            .check_p2id_notes_exist(&[&p2id1_note_id, &p2id2_note_id])
            .await
        {
            if !existing.is_empty() {
                return Err(anyhow::anyhow!(
                    "P2ID notes already exist for this match: {:?}. Concurrent matching detected.",
                    existing
                ));
            }
        }

        /* ---------- 2. Fill calculation ---------- */
        let p2id1_assets = swap_data.p2id_from_1_to_2.assets();
        let executed_quantity = p2id1_assets
            .iter()
            .next()
            .map(|a| a.unwrap_fungible().amount() as f64)
            .unwrap_or(0.0);

        let executed_price = (order1.price + order2.price) / 2.0;

        /* ---------- 3. Update DB ---------- */
        let order1_status = if swap_data.leftover_swapp_note.is_some() {
            SwapNoteStatus::PartiallyFilled
        } else {
            SwapNoteStatus::Filled
        };

        db.update_swap_note_status(&order1.note_id, order1_status)
            .await?;
        db.update_swap_note_status(&order2.note_id, SwapNoteStatus::Filled)
            .await?;

        /* ---------- 4. Insert P2ID notes ---------- */
        let (p2id1_id, p2id2_id) = {
            let p2id1_data = serialize_note(&swap_data.p2id_from_1_to_2)?;
            let p2id2_data = serialize_note(&swap_data.p2id_from_2_to_1)?;
            let p2id1_id = Uuid::new_v4().to_string();
            let p2id2_id = Uuid::new_v4().to_string();

            let p2id1_record = P2IdNoteRecord {
                id: p2id1_id.clone(),
                note_id: p2id1_note_id.clone(),
                sender_id: "matcher".into(),
                target_id: order1.creator_id.clone(),
                asset_id: order2.offered_asset_id.clone(),
                amount: executed_quantity as u64,
                swap_note_id: Some(order1.id.clone()),
                note_data: p2id1_data,
                created_at: Utc::now(),
            };

            let p2id2_record = P2IdNoteRecord {
                id: p2id2_id.clone(),
                note_id: p2id2_note_id.clone(),
                sender_id: "matcher".into(),
                target_id: order2.creator_id.clone(),
                asset_id: order1.offered_asset_id.clone(),
                amount: executed_quantity as u64,
                swap_note_id: Some(order2.id.clone()),
                note_data: p2id2_data,
                created_at: Utc::now(),
            };

            // Insert with duplicate-check handling
            db.insert_p2id_note(&p2id1_record)
                .await
                .map_err(|e| anyhow::anyhow!("Insert P2ID-1 failed: {}", e))?;
            db.insert_p2id_note(&p2id2_record)
                .await
                .map_err(|e| anyhow::anyhow!("Insert P2ID-2 failed: {}", e))?;

            (p2id1_id, p2id2_id)
        };

        /* ---------- 5. Execute blockchain tx *inline* (no cross-thread move) ---------- */
        if let (Some(note1), Some(note2)) = (
            self.open_orders.get(&order1.note_id).cloned(),
            self.open_orders.get(&order2.note_id).cloned(),
        ) {
            let endpoint = {
                let s = env::var("MIDEN_NODE_ENDPOINT")
                    .map_err(|_| anyhow::anyhow!("MIDEN_NODE_ENDPOINT not set"))?;
                Endpoint::try_from(s.as_str())
                    .map_err(|_| anyhow::anyhow!("Invalid MIDEN_NODE_ENDPOINT"))?
            };

            // Execute blockchain transaction inline (same thread)
            match execute_match_transaction(&note1, &note2, swap_data, matcher_id, endpoint).await {
                Ok(tx) => info!("🚀 blockchain tx sent: {}", tx),
                Err(e) => error!("❌ blockchain tx failed: {}", e),
            }
        } else {
            warn!("❌ Notes not found in cache – skipping blockchain execution");
        }

        /* ---------- 6. Handle leftover note (if any) ---------- */
        let leftover_id = if let Some(ref leftover) = swap_data.leftover_swapp_note {
            let data = serialize_note(leftover)?;
            let (
                creator_id,
                offered_asset_id,
                offered_amount,
                requested_asset_id,
                requested_amount,
                price,
                is_bid,
            ) = extract_note_info(leftover)?;

            let rec = SwapNoteRecord {
                id: Uuid::new_v4().to_string(),
                note_id: leftover.id().to_hex(),
                creator_id,
                offered_asset_id,
                offered_amount,
                requested_asset_id,
                requested_amount,
                price,
                is_bid,
                note_data: data,
                status: SwapNoteStatus::Open,
                failure_count: 0,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };

            db.insert_swap_note(&rec).await?;
            self.open_orders
                .insert(leftover.id().to_hex(), leftover.clone());
            Some(rec.id)
        } else {
            None
        };

        /* ---------- 7. Return result ---------- */
        Ok(MatchResult {
            order1_id: order1.id.clone(),
            order2_id: order2.id.clone(),
            executed_price,
            executed_quantity,
            p2id_notes: vec![p2id1_id, p2id2_id],
            leftover_order: leftover_id,
        })
    }

    // Keep the old method for backward compatibility but mark as deprecated
    async fn execute_match(
        &mut self,
        order1: &SwapNoteRecord,
        order2: &SwapNoteRecord,
        db: &Database,
    ) -> Result<MatchResult> {
        warn!(
            "Using deprecated execute_match method. This should be replaced with execute_real_match."
        );

        // Calculate match details (simplified)
        let executed_price = (order1.price + order2.price) / 2.0;
        let executed_quantity =
            std::cmp::min(order1.offered_amount, order2.requested_amount) as f64;

        // Update order statuses in database
        db.update_swap_note_status(&order1.note_id, SwapNoteStatus::Filled)
            .await?;
        db.update_swap_note_status(&order2.note_id, SwapNoteStatus::Filled)
            .await?;

        // Remove from in-memory cache
        self.open_orders.remove(&order1.note_id);
        self.open_orders.remove(&order2.note_id);

        // Create P2ID note records (simplified)
        let p2id1_id = Uuid::new_v4().to_string();
        let p2id2_id = Uuid::new_v4().to_string();

        let p2id1_record = P2IdNoteRecord {
            id: p2id1_id.clone(),
            note_id: format!("p2id_{}", Uuid::new_v4()),
            sender_id: "matcher".to_string(),
            target_id: order1.creator_id.clone(),
            asset_id: order2.offered_asset_id.clone(),
            amount: executed_quantity as u64,
            swap_note_id: Some(order1.id.clone()),
            note_data: "{}".to_string(),
            created_at: Utc::now(),
        };

        let p2id2_record = P2IdNoteRecord {
            id: p2id2_id.clone(),
            note_id: format!("p2id_{}", Uuid::new_v4()),
            sender_id: "matcher".to_string(),
            target_id: order2.creator_id.clone(),
            asset_id: order1.offered_asset_id.clone(),
            amount: executed_quantity as u64,
            swap_note_id: Some(order2.id.clone()),
            note_data: "{}".to_string(),
            created_at: Utc::now(),
        };

        // Store P2ID notes in database
        db.insert_p2id_note(&p2id1_record).await?;
        db.insert_p2id_note(&p2id2_record).await?;

        let match_result = MatchResult {
            order1_id: order1.id.clone(),
            order2_id: order2.id.clone(),
            executed_price,
            executed_quantity,
            p2id_notes: vec![p2id1_id, p2id2_id],
            leftover_order: None,
        };

        info!("Successfully executed match: {:?}", match_result);
        Ok(match_result)
    }

    pub fn get_open_order_count(&self) -> usize {
        self.open_orders.len()
    }
}
