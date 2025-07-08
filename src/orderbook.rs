use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;
use uuid::Uuid;

use miden_client::note::Note;

use crate::{
    database::{Database, SwapNoteRecord, SwapNoteStatus},
    note_serialization::{extract_note_info, serialize_note},
};

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
        // stubbed out for now
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
}
