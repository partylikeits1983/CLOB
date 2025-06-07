use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::fs;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SwapNoteRecord {
    pub id: String,
    pub note_id: String,
    pub creator_id: String,
    pub offered_asset_id: String,
    pub offered_amount: u64,
    pub requested_asset_id: String,
    pub requested_amount: u64,
    pub price: f64,
    pub is_bid: bool,
    pub note_data: String, // Serialized note
    pub status: SwapNoteStatus,
    pub failure_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FailedSwapNoteRecord {
    pub id: String,
    pub note_id: String,
    pub creator_id: String,
    pub offered_asset_id: String,
    pub offered_amount: u64,
    pub requested_asset_id: String,
    pub requested_amount: u64,
    pub price: f64,
    pub is_bid: bool,
    pub note_data: String, // Serialized note
    pub failure_count: i32,
    pub failure_reason: String,
    pub created_at: DateTime<Utc>,
    pub failed_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct P2IdNoteRecord {
    pub id: String,
    pub note_id: String,
    pub sender_id: String,
    pub target_id: String,
    pub recipient: String, // Recipient account id as hex
    pub asset_id: String,
    pub amount: u64,
    pub swap_note_id: Option<String>,
    pub note_data: String, // Serialized note
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FilledOrderRecord {
    pub id: String,
    pub note_id: String,
    pub creator_id: String,
    pub offered_asset_id: String,
    pub offered_amount: u64,
    pub requested_asset_id: String,
    pub requested_amount: u64,
    pub price: f64,
    pub is_bid: bool,
    pub note_data: String, // Serialized note
    pub original_status: SwapNoteStatus, // The status before being filled (Open, PartiallyFilled)
    pub fill_type: String, // "complete" or "partial"
    pub transaction_id: Option<String>, // The blockchain transaction ID that filled this order
    pub created_at: DateTime<Utc>,
    pub filled_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum SwapNoteStatus {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
}

impl std::fmt::Display for SwapNoteStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapNoteStatus::Open => write!(f, "open"),
            SwapNoteStatus::PartiallyFilled => write!(f, "partially_filled"),
            SwapNoteStatus::Filled => write!(f, "filled"),
            SwapNoteStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::str::FromStr for SwapNoteStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "open" => Ok(SwapNoteStatus::Open),
            "partially_filled" => Ok(SwapNoteStatus::PartiallyFilled),
            "filled" => Ok(SwapNoteStatus::Filled),
            "cancelled" => Ok(SwapNoteStatus::Cancelled),
            _ => Err(anyhow::anyhow!("Invalid swap note status: {}", s)),
        }
    }
}

pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self> {
        // Ensure the database file can be created
        if database_url.starts_with("sqlite:") {
            let file_path = database_url.strip_prefix("sqlite:").unwrap_or(database_url);
            let file_path = file_path.split('?').next().unwrap_or(file_path);

            // Create parent directory if it doesn't exist
            if let Some(parent) = std::path::Path::new(file_path).parent() {
                fs::create_dir_all(parent)?;
            }

            println!("Database file path: {}", file_path);
        }

        println!("Connecting to database: {}", database_url);
        let pool = SqlitePool::connect(database_url).await?;

        let db = Self { pool };
        println!("Running database migration...");
        db.migrate().await?;
        println!("Database migration completed successfully!");

        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
        println!("Creating swap_notes table...");
        let result = sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS swap_notes (
                id TEXT PRIMARY KEY,
                note_id TEXT UNIQUE NOT NULL,
                creator_id TEXT NOT NULL,
                offered_asset_id TEXT NOT NULL,
                offered_amount INTEGER NOT NULL,
                requested_asset_id TEXT NOT NULL,
                requested_amount INTEGER NOT NULL,
                price REAL NOT NULL,
                is_bid BOOLEAN NOT NULL,
                note_data TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'open',
                failure_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => println!("✅ swap_notes table created successfully"),
            Err(e) => {
                println!("❌ Failed to create swap_notes table: {}", e);
                return Err(e.into());
            }
        }

        // Add failure_count column to existing tables if it doesn't exist
        println!("Adding failure_count column to existing swap_notes table...");
        let _ = sqlx::query("ALTER TABLE swap_notes ADD COLUMN failure_count INTEGER DEFAULT 0")
            .execute(&self.pool)
            .await; // Ignore error if column already exists

        // Update existing records to have failure_count = 0 if they don't have it
        println!("Updating existing records to have failure_count = 0...");
        let _ = sqlx::query("UPDATE swap_notes SET failure_count = 0 WHERE failure_count IS NULL")
            .execute(&self.pool)
            .await; // Ignore error if column doesn't exist or no records need updating

        println!("Creating failed_swap_notes table...");
        let result = sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS failed_swap_notes (
                id TEXT PRIMARY KEY,
                note_id TEXT UNIQUE NOT NULL,
                creator_id TEXT NOT NULL,
                offered_asset_id TEXT NOT NULL,
                offered_amount INTEGER NOT NULL,
                requested_asset_id TEXT NOT NULL,
                requested_amount INTEGER NOT NULL,
                price REAL NOT NULL,
                is_bid BOOLEAN NOT NULL,
                note_data TEXT NOT NULL,
                failure_count INTEGER NOT NULL,
                failure_reason TEXT NOT NULL,
                created_at TEXT NOT NULL,
                failed_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => println!("✅ failed_swap_notes table created successfully"),
            Err(e) => {
                println!("❌ Failed to create failed_swap_notes table: {}", e);
                return Err(e.into());
            }
        }

        println!("Creating p2id_notes table...");
        let result = sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS p2id_notes (
                id TEXT PRIMARY KEY,
                note_id TEXT UNIQUE NOT NULL,
                sender_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                recipient TEXT NOT NULL,
                asset_id TEXT NOT NULL,
                amount INTEGER NOT NULL,
                swap_note_id TEXT,
                note_data TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => println!("✅ p2id_notes table created successfully"),
            Err(e) => {
                println!("❌ Failed to create p2id_notes table: {}", e);
                return Err(e.into());
            }
        }

        // Add recipient column to existing p2id_notes table if it doesn't exist
        println!("Adding recipient column to existing p2id_notes table...");
        let _ = sqlx::query("ALTER TABLE p2id_notes ADD COLUMN recipient TEXT")
            .execute(&self.pool)
            .await; // Ignore error if column already exists

        // Drop the foreign key constraint if it exists (we need to recreate the table for SQLite)
        println!("Recreating p2id_notes table without foreign key constraint...");
        let _ = sqlx::query("DROP TABLE IF EXISTS p2id_notes_backup").execute(&self.pool).await;
        let _ = sqlx::query("ALTER TABLE p2id_notes RENAME TO p2id_notes_backup").execute(&self.pool).await;
        
        // Recreate table without foreign key constraint
        let _ = sqlx::query(
            r#"
            CREATE TABLE p2id_notes (
                id TEXT PRIMARY KEY,
                note_id TEXT UNIQUE NOT NULL,
                sender_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                recipient TEXT,
                asset_id TEXT NOT NULL,
                amount INTEGER NOT NULL,
                swap_note_id TEXT,
                note_data TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        ).execute(&self.pool).await;
        
        // Copy data from backup table
        let _ = sqlx::query(
            r#"
            INSERT INTO p2id_notes (id, note_id, sender_id, target_id, recipient, asset_id, amount, swap_note_id, note_data, created_at)
            SELECT id, note_id, sender_id, target_id,
                   CASE WHEN recipient IS NOT NULL THEN recipient ELSE target_id END as recipient,
                   asset_id, amount, swap_note_id, note_data, created_at
            FROM p2id_notes_backup
            "#,
        ).execute(&self.pool).await;
        
        // Drop backup table
        let _ = sqlx::query("DROP TABLE IF EXISTS p2id_notes_backup").execute(&self.pool).await;

        println!("Creating filled_orders table...");
        let result = sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS filled_orders (
                id TEXT PRIMARY KEY,
                note_id TEXT NOT NULL,
                creator_id TEXT NOT NULL,
                offered_asset_id TEXT NOT NULL,
                offered_amount INTEGER NOT NULL,
                requested_asset_id TEXT NOT NULL,
                requested_amount INTEGER NOT NULL,
                price REAL NOT NULL,
                is_bid BOOLEAN NOT NULL,
                note_data TEXT NOT NULL,
                original_status TEXT NOT NULL,
                fill_type TEXT NOT NULL,
                transaction_id TEXT,
                created_at TEXT NOT NULL,
                filled_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => println!("✅ filled_orders table created successfully"),
            Err(e) => {
                println!("❌ Failed to create filled_orders table: {}", e);
                return Err(e.into());
            }
        }

        // Create indexes for better performance
        println!("Creating database indexes...");
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_swap_notes_status ON swap_notes(status)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_swap_notes_creator ON swap_notes(creator_id)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_swap_notes_assets ON swap_notes(offered_asset_id, requested_asset_id)")
            .execute(&self.pool)
            .await?;

        println!("✅ All database indexes created successfully");

        // Verify tables exist
        let table_check = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('swap_notes', 'p2id_notes')")
            .fetch_all(&self.pool)
            .await?;

        println!("📊 Found {} tables in database", table_check.len());
        for row in table_check {
            let table_name: String = row.get("name");
            println!("  - Table: {}", table_name);
        }

        Ok(())
    }

    pub async fn insert_swap_note(&self, record: &SwapNoteRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO swap_notes (
                id, note_id, creator_id, offered_asset_id, offered_amount,
                requested_asset_id, requested_amount, price, is_bid,
                note_data, status, failure_count, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&record.id)
        .bind(&record.note_id)
        .bind(&record.creator_id)
        .bind(&record.offered_asset_id)
        .bind(record.offered_amount as i64)
        .bind(&record.requested_asset_id)
        .bind(record.requested_amount as i64)
        .bind(record.price)
        .bind(record.is_bid)
        .bind(&record.note_data)
        .bind(record.status.to_string())
        .bind(record.failure_count)
        .bind(record.created_at.to_rfc3339())
        .bind(record.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn insert_p2id_note(&self, record: &P2IdNoteRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO p2id_notes (
                id, note_id, sender_id, target_id, recipient, asset_id, amount,
                swap_note_id, note_data, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&record.id)
        .bind(&record.note_id)
        .bind(&record.sender_id)
        .bind(&record.target_id)
        .bind(&record.recipient)
        .bind(&record.asset_id)
        .bind(record.amount as i64)
        .bind(&record.swap_note_id)
        .bind(&record.note_data)
        .bind(record.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_open_swap_notes(&self) -> Result<Vec<SwapNoteRecord>> {
        let rows =
            sqlx::query("SELECT * FROM swap_notes WHERE status = 'open' ORDER BY created_at ASC")
                .fetch_all(&self.pool)
                .await?;

        let mut records = Vec::new();
        for row in rows {
            let status_str: String = row.get("status");
            let created_at_str: String = row.get("created_at");
            let updated_at_str: String = row.get("updated_at");

            records.push(SwapNoteRecord {
                id: row.get("id"),
                note_id: row.get("note_id"),
                creator_id: row.get("creator_id"),
                offered_asset_id: row.get("offered_asset_id"),
                offered_amount: row.get::<i64, _>("offered_amount") as u64,
                requested_asset_id: row.get("requested_asset_id"),
                requested_amount: row.get::<i64, _>("requested_amount") as u64,
                price: row.get("price"),
                is_bid: row.get("is_bid"),
                note_data: row.get("note_data"),
                status: status_str.parse()?,
                failure_count: row.try_get("failure_count").unwrap_or(0),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)?.with_timezone(&Utc),
            });
        }

        Ok(records)
    }

    pub async fn update_swap_note_status(
        &self,
        note_id: &str,
        status: SwapNoteStatus,
    ) -> Result<()> {
        sqlx::query("UPDATE swap_notes SET status = ?, updated_at = ? WHERE note_id = ?")
            .bind(status.to_string())
            .bind(Utc::now().to_rfc3339())
            .bind(note_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_orderbook(
        &self,
        base_asset: &str,
        quote_asset: &str,
    ) -> Result<(Vec<SwapNoteRecord>, Vec<SwapNoteRecord>)> {
        // Get bids (buying base asset with quote asset)
        let bid_rows = sqlx::query(
            "SELECT * FROM swap_notes WHERE status = 'open' AND offered_asset_id = ? AND requested_asset_id = ? ORDER BY price DESC"
        )
        .bind(quote_asset)
        .bind(base_asset)
        .fetch_all(&self.pool)
        .await?;

        // Get asks (selling base asset for quote asset)
        let ask_rows = sqlx::query(
            "SELECT * FROM swap_notes WHERE status = 'open' AND offered_asset_id = ? AND requested_asset_id = ? ORDER BY price ASC"
        )
        .bind(base_asset)
        .bind(quote_asset)
        .fetch_all(&self.pool)
        .await?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for row in bid_rows {
            let status_str: String = row.get("status");
            let created_at_str: String = row.get("created_at");
            let updated_at_str: String = row.get("updated_at");

            bids.push(SwapNoteRecord {
                id: row.get("id"),
                note_id: row.get("note_id"),
                creator_id: row.get("creator_id"),
                offered_asset_id: row.get("offered_asset_id"),
                offered_amount: row.get::<i64, _>("offered_amount") as u64,
                requested_asset_id: row.get("requested_asset_id"),
                requested_amount: row.get::<i64, _>("requested_amount") as u64,
                price: row.get("price"),
                is_bid: row.get("is_bid"),
                note_data: row.get("note_data"),
                status: status_str.parse()?,
                failure_count: row.try_get("failure_count").unwrap_or(0),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)?.with_timezone(&Utc),
            });
        }

        for row in ask_rows {
            let status_str: String = row.get("status");
            let created_at_str: String = row.get("created_at");
            let updated_at_str: String = row.get("updated_at");

            asks.push(SwapNoteRecord {
                id: row.get("id"),
                note_id: row.get("note_id"),
                creator_id: row.get("creator_id"),
                offered_asset_id: row.get("offered_asset_id"),
                offered_amount: row.get::<i64, _>("offered_amount") as u64,
                requested_asset_id: row.get("requested_asset_id"),
                requested_amount: row.get::<i64, _>("requested_amount") as u64,
                price: row.get("price"),
                is_bid: row.get("is_bid"),
                note_data: row.get("note_data"),
                status: status_str.parse()?,
                failure_count: row.try_get("failure_count").unwrap_or(0),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)?.with_timezone(&Utc),
            });
        }

        Ok((bids, asks))
    }

    pub async fn get_user_orders(&self, user_id: &str) -> Result<Vec<SwapNoteRecord>> {
        let rows =
            sqlx::query("SELECT * FROM swap_notes WHERE creator_id = ? ORDER BY created_at DESC")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;

        let mut records = Vec::new();
        for row in rows {
            let status_str: String = row.get("status");
            let created_at_str: String = row.get("created_at");
            let updated_at_str: String = row.get("updated_at");

            records.push(SwapNoteRecord {
                id: row.get("id"),
                note_id: row.get("note_id"),
                creator_id: row.get("creator_id"),
                offered_asset_id: row.get("offered_asset_id"),
                offered_amount: row.get::<i64, _>("offered_amount") as u64,
                requested_asset_id: row.get("requested_asset_id"),
                requested_amount: row.get::<i64, _>("requested_amount") as u64,
                price: row.get("price"),
                is_bid: row.get("is_bid"),
                note_data: row.get("note_data"),
                status: status_str.parse()?,
                failure_count: row.try_get("failure_count").unwrap_or(0),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)?.with_timezone(&Utc),
            });
        }

        Ok(records)
    }

    pub async fn check_p2id_notes_exist(&self, note_ids: &[&str]) -> Result<Vec<String>> {
        if note_ids.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = note_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT note_id FROM p2id_notes WHERE note_id IN ({})",
            placeholders
        );

        let mut query_builder = sqlx::query(&query);
        for note_id in note_ids {
            query_builder = query_builder.bind(*note_id);
        }

        let rows = query_builder.fetch_all(&self.pool).await?;
        let existing_ids: Vec<String> = rows.iter().map(|row| row.get("note_id")).collect();

        Ok(existing_ids)
    }

    pub async fn increment_failure_count(&self, note_id: &str) -> Result<i32> {
        let result = sqlx::query(
            "UPDATE swap_notes SET failure_count = failure_count + 1, updated_at = ? WHERE note_id = ? RETURNING failure_count"
        )
        .bind(Utc::now().to_rfc3339())
        .bind(note_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(result.get("failure_count"))
    }

    pub async fn move_to_failed_table(&self, note_id: &str, failure_reason: &str) -> Result<()> {
        // First, get the swap note record
        let swap_note = sqlx::query("SELECT * FROM swap_notes WHERE note_id = ?")
            .bind(note_id)
            .fetch_one(&self.pool)
            .await?;

        // Insert into failed_swap_notes table
        sqlx::query(
            r#"
            INSERT INTO failed_swap_notes (
                id, note_id, creator_id, offered_asset_id, offered_amount,
                requested_asset_id, requested_amount, price, is_bid,
                note_data, failure_count, failure_reason, created_at, failed_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(swap_note.get::<String, _>("id"))
        .bind(swap_note.get::<String, _>("note_id"))
        .bind(swap_note.get::<String, _>("creator_id"))
        .bind(swap_note.get::<String, _>("offered_asset_id"))
        .bind(swap_note.get::<i64, _>("offered_amount"))
        .bind(swap_note.get::<String, _>("requested_asset_id"))
        .bind(swap_note.get::<i64, _>("requested_amount"))
        .bind(swap_note.get::<f64, _>("price"))
        .bind(swap_note.get::<bool, _>("is_bid"))
        .bind(swap_note.get::<String, _>("note_data"))
        .bind(swap_note.try_get::<i32, _>("failure_count").unwrap_or(0))
        .bind(failure_reason)
        .bind(swap_note.get::<String, _>("created_at"))
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;

        // Remove from swap_notes table
        sqlx::query("DELETE FROM swap_notes WHERE note_id = ?")
            .bind(note_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_failed_swap_notes(&self) -> Result<Vec<FailedSwapNoteRecord>> {
        let rows = sqlx::query("SELECT * FROM failed_swap_notes ORDER BY failed_at DESC")
            .fetch_all(&self.pool)
            .await?;

        let mut records = Vec::new();
        for row in rows {
            let created_at_str: String = row.get("created_at");
            let failed_at_str: String = row.get("failed_at");

            records.push(FailedSwapNoteRecord {
                id: row.get("id"),
                note_id: row.get("note_id"),
                creator_id: row.get("creator_id"),
                offered_asset_id: row.get("offered_asset_id"),
                offered_amount: row.get::<i64, _>("offered_amount") as u64,
                requested_asset_id: row.get("requested_asset_id"),
                requested_amount: row.get::<i64, _>("requested_amount") as u64,
                price: row.get("price"),
                is_bid: row.get("is_bid"),
                note_data: row.get("note_data"),
                failure_count: row.get("failure_count"),
                failure_reason: row.get("failure_reason"),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
                failed_at: DateTime::parse_from_rfc3339(&failed_at_str)?.with_timezone(&Utc),
            });
        }

        Ok(records)
    }

    pub async fn insert_filled_order(&self, record: &FilledOrderRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO filled_orders (
                id, note_id, creator_id, offered_asset_id, offered_amount,
                requested_asset_id, requested_amount, price, is_bid,
                note_data, original_status, fill_type, transaction_id, created_at, filled_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&record.id)
        .bind(&record.note_id)
        .bind(&record.creator_id)
        .bind(&record.offered_asset_id)
        .bind(record.offered_amount as i64)
        .bind(&record.requested_asset_id)
        .bind(record.requested_amount as i64)
        .bind(record.price)
        .bind(record.is_bid)
        .bind(&record.note_data)
        .bind(record.original_status.to_string())
        .bind(&record.fill_type)
        .bind(&record.transaction_id)
        .bind(record.created_at.to_rfc3339())
        .bind(record.filled_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn move_to_filled_orders(&self, note_id: &str, fill_type: &str, transaction_id: Option<String>) -> Result<()> {
        // First, get the swap note record
        let swap_note = sqlx::query("SELECT * FROM swap_notes WHERE note_id = ?")
            .bind(note_id)
            .fetch_one(&self.pool)
            .await?;

        // Parse the status from the database
        let status_str: String = swap_note.get("status");
        let original_status: SwapNoteStatus = status_str.parse()?;

        // Create a filled order record
        let filled_record = FilledOrderRecord {
            id: Uuid::new_v4().to_string(),
            note_id: swap_note.get::<String, _>("note_id"),
            creator_id: swap_note.get::<String, _>("creator_id"),
            offered_asset_id: swap_note.get::<String, _>("offered_asset_id"),
            offered_amount: swap_note.get::<i64, _>("offered_amount") as u64,
            requested_asset_id: swap_note.get::<String, _>("requested_asset_id"),
            requested_amount: swap_note.get::<i64, _>("requested_amount") as u64,
            price: swap_note.get::<f64, _>("price"),
            is_bid: swap_note.get::<bool, _>("is_bid"),
            note_data: swap_note.get::<String, _>("note_data"),
            original_status,
            fill_type: fill_type.to_string(),
            transaction_id,
            created_at: {
                let created_at_str: String = swap_note.get("created_at");
                DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc)
            },
            filled_at: Utc::now(),
        };

        // Insert into filled_orders table
        self.insert_filled_order(&filled_record).await?;

        // Remove from swap_notes table
        sqlx::query("DELETE FROM swap_notes WHERE note_id = ?")
            .bind(note_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_filled_orders(&self) -> Result<Vec<FilledOrderRecord>> {
        let rows = sqlx::query("SELECT * FROM filled_orders ORDER BY filled_at DESC")
            .fetch_all(&self.pool)
            .await?;

        let mut records = Vec::new();
        for row in rows {
            let original_status_str: String = row.get("original_status");
            let created_at_str: String = row.get("created_at");
            let filled_at_str: String = row.get("filled_at");

            records.push(FilledOrderRecord {
                id: row.get("id"),
                note_id: row.get("note_id"),
                creator_id: row.get("creator_id"),
                offered_asset_id: row.get("offered_asset_id"),
                offered_amount: row.get::<i64, _>("offered_amount") as u64,
                requested_asset_id: row.get("requested_asset_id"),
                requested_amount: row.get::<i64, _>("requested_amount") as u64,
                price: row.get("price"),
                is_bid: row.get("is_bid"),
                note_data: row.get("note_data"),
                original_status: original_status_str.parse()?,
                fill_type: row.get("fill_type"),
                transaction_id: row.get("transaction_id"),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
                filled_at: DateTime::parse_from_rfc3339(&filled_at_str)?.with_timezone(&Utc),
            });
        }

        Ok(records)
    }

    pub async fn get_user_filled_orders(&self, user_id: &str) -> Result<Vec<FilledOrderRecord>> {
        let rows = sqlx::query("SELECT * FROM filled_orders WHERE creator_id = ? ORDER BY filled_at DESC")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;

        let mut records = Vec::new();
        for row in rows {
            let original_status_str: String = row.get("original_status");
            let created_at_str: String = row.get("created_at");
            let filled_at_str: String = row.get("filled_at");

            records.push(FilledOrderRecord {
                id: row.get("id"),
                note_id: row.get("note_id"),
                creator_id: row.get("creator_id"),
                offered_asset_id: row.get("offered_asset_id"),
                offered_amount: row.get::<i64, _>("offered_amount") as u64,
                requested_asset_id: row.get("requested_asset_id"),
                requested_amount: row.get::<i64, _>("requested_amount") as u64,
                price: row.get("price"),
                is_bid: row.get("is_bid"),
                note_data: row.get("note_data"),
                original_status: original_status_str.parse()?,
                fill_type: row.get("fill_type"),
                transaction_id: row.get("transaction_id"),
                created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
                filled_at: DateTime::parse_from_rfc3339(&filled_at_str)?.with_timezone(&Utc),
            });
        }

        Ok(records)
    }
}
