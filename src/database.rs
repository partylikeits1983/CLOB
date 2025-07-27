use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::{Arc, Mutex};
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
    pub note_data: String,               // Serialized note
    pub original_status: SwapNoteStatus, // The status before being filled (Open, PartiallyFilled)
    pub fill_type: String,               // "complete" or "partial"
    pub transaction_id: Option<String>,  // The blockchain transaction ID that filled this order
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
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self> {
        // Extract file path from database URL
        let file_path = if database_url.starts_with("sqlite:") {
            database_url.strip_prefix("sqlite:").unwrap_or(database_url)
        } else {
            database_url
        };
        let file_path = file_path.split('?').next().unwrap_or(file_path);

        // Create parent directory if it doesn't exist
        if let Some(parent) = std::path::Path::new(file_path).parent() {
            fs::create_dir_all(parent)?;
        }

        println!("Database file path: {}", file_path);
        println!("Connecting to database: {}", database_url);

        let conn = Connection::open(file_path)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        // Configure SQLite for better consistency and performance
        println!("Configuring SQLite settings...");
        db.execute_pragma("PRAGMA journal_mode = WAL")?;
        db.execute_pragma("PRAGMA synchronous = NORMAL")?;
        db.execute_pragma("PRAGMA cache_size = 1000")?;
        db.execute_pragma("PRAGMA temp_store = memory")?;

        println!("Running database migration...");
        db.migrate().await?;
        println!("Database migration completed successfully!");

        Ok(db)
    }

    fn execute_pragma(&self, pragma: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(pragma)?;
        let _ = stmt.query(())?;
        Ok(())
    }

    async fn migrate(&self) -> Result<()> {
        println!("Creating swap_notes table...");
        let result = self.execute(
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
            (),
        );

        match result {
            Ok(_) => println!("✅ swap_notes table created successfully"),
            Err(e) => {
                println!("❌ Failed to create swap_notes table: {}", e);
                return Err(e.into());
            }
        }

        // Add failure_count column to existing tables if it doesn't exist
        println!("Adding failure_count column to existing swap_notes table...");
        let _ = self.execute(
            "ALTER TABLE swap_notes ADD COLUMN failure_count INTEGER DEFAULT 0",
            (),
        );

        // Update existing records to have failure_count = 0 if they don't have it
        println!("Updating existing records to have failure_count = 0...");
        let _ = self.execute(
            "UPDATE swap_notes SET failure_count = 0 WHERE failure_count IS NULL",
            (),
        );

        println!("Creating failed_swap_notes table...");
        let result = self.execute(
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
            (),
        );

        match result {
            Ok(_) => println!("✅ failed_swap_notes table created successfully"),
            Err(e) => {
                println!("❌ Failed to create failed_swap_notes table: {}", e);
                return Err(e.into());
            }
        }

        println!("Creating p2id_notes table...");
        let result = self.execute(
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
            (),
        );

        match result {
            Ok(_) => println!("✅ p2id_notes table created successfully"),
            Err(e) => {
                println!("❌ Failed to create p2id_notes table: {}", e);
                return Err(e.into());
            }
        }

        // Add recipient column to existing p2id_notes table if it doesn't exist
        println!("Adding recipient column to existing p2id_notes table...");
        let _ = self.execute("ALTER TABLE p2id_notes ADD COLUMN recipient TEXT", ());

        // Drop the foreign key constraint if it exists (we need to recreate the table for SQLite)
        println!("Recreating p2id_notes table without foreign key constraint...");
        let _ = self.execute("DROP TABLE IF EXISTS p2id_notes_backup", ());
        let _ = self.execute("ALTER TABLE p2id_notes RENAME TO p2id_notes_backup", ());

        // Recreate table without foreign key constraint
        let _ = self.execute(
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
            (),
        );

        // Copy data from backup table
        let _ = self.execute(
            r#"
            INSERT INTO p2id_notes (id, note_id, sender_id, target_id, recipient, asset_id, amount, swap_note_id, note_data, created_at)
            SELECT id, note_id, sender_id, target_id,
                   CASE WHEN recipient IS NOT NULL THEN recipient ELSE target_id END as recipient,
                   asset_id, amount, swap_note_id, note_data, created_at
            FROM p2id_notes_backup
            "#,
            (),
        );

        // Drop backup table
        let _ = self.execute("DROP TABLE IF EXISTS p2id_notes_backup", ());

        println!("Creating filled_orders table...");
        let result = self.execute(
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
            (),
        );

        match result {
            Ok(_) => println!("✅ filled_orders table created successfully"),
            Err(e) => {
                println!("❌ Failed to create filled_orders table: {}", e);
                return Err(e.into());
            }
        }

        // Create indexes for better performance
        println!("Creating database indexes...");
        self.execute(
            "CREATE INDEX IF NOT EXISTS idx_swap_notes_status ON swap_notes(status)",
            (),
        )?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS idx_swap_notes_creator ON swap_notes(creator_id)",
            (),
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_swap_notes_assets ON swap_notes(offered_asset_id, requested_asset_id)", ())?;

        println!("✅ All database indexes created successfully");

        // Verify tables exist
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('swap_notes', 'p2id_notes')")?;
        let table_iter = stmt.query_map([], |row| Ok(row.get::<_, String>(0)?))?;

        let mut table_count = 0;
        for table_result in table_iter {
            let table_name = table_result?;
            println!("  - Table: {}", table_name);
            table_count += 1;
        }

        println!("📊 Found {} tables in database", table_count);

        Ok(())
    }

    fn execute<P>(&self, sql: &str, params: P) -> Result<usize>
    where
        P: rusqlite::Params,
    {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(sql, params)?)
    }

    pub async fn insert_swap_note(&self, record: &SwapNoteRecord) -> Result<()> {
        self.execute(
            r#"
            INSERT INTO swap_notes (
                id, note_id, creator_id, offered_asset_id, offered_amount,
                requested_asset_id, requested_amount, price, is_bid,
                note_data, status, failure_count, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            params![
                &record.id,
                &record.note_id,
                &record.creator_id,
                &record.offered_asset_id,
                record.offered_amount as i64,
                &record.requested_asset_id,
                record.requested_amount as i64,
                record.price,
                record.is_bid,
                &record.note_data,
                record.status.to_string(),
                record.failure_count,
                record.created_at.to_rfc3339(),
                record.updated_at.to_rfc3339()
            ],
        )?;

        Ok(())
    }

    pub async fn insert_p2id_note(&self, record: &P2IdNoteRecord) -> Result<()> {
        self.execute(
            r#"
            INSERT INTO p2id_notes (
                id, note_id, sender_id, target_id, recipient, asset_id, amount,
                swap_note_id, note_data, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                &record.id,
                &record.note_id,
                &record.sender_id,
                &record.target_id,
                &record.recipient,
                &record.asset_id,
                record.amount as i64,
                &record.swap_note_id,
                &record.note_data,
                record.created_at.to_rfc3339()
            ],
        )?;

        Ok(())
    }

    fn row_to_swap_note_record(row: &Row) -> Result<SwapNoteRecord> {
        let status_str: String = row.get("status")?;
        let created_at_str: String = row.get("created_at")?;
        let updated_at_str: String = row.get("updated_at")?;

        Ok(SwapNoteRecord {
            id: row.get("id")?,
            note_id: row.get("note_id")?,
            creator_id: row.get("creator_id")?,
            offered_asset_id: row.get("offered_asset_id")?,
            offered_amount: row.get::<_, i64>("offered_amount")? as u64,
            requested_asset_id: row.get("requested_asset_id")?,
            requested_amount: row.get::<_, i64>("requested_amount")? as u64,
            price: row.get("price")?,
            is_bid: row.get("is_bid")?,
            note_data: row.get("note_data")?,
            status: status_str.parse()?,
            failure_count: row.get("failure_count").unwrap_or(0),
            created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at_str)?.with_timezone(&Utc),
        })
    }

    pub async fn get_open_swap_notes(&self) -> Result<Vec<SwapNoteRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT * FROM swap_notes WHERE status = 'open' ORDER BY created_at ASC")?;
        let row_iter = stmt.query_map([], |row| Ok(Self::row_to_swap_note_record(row).unwrap()))?;

        let mut records = Vec::new();
        for record_result in row_iter {
            records.push(record_result?);
        }

        Ok(records)
    }

    pub async fn update_swap_note_status(
        &self,
        note_id: &str,
        status: SwapNoteStatus,
    ) -> Result<()> {
        self.execute(
            "UPDATE swap_notes SET status = ?1, updated_at = ?2 WHERE note_id = ?3",
            params![status.to_string(), Utc::now().to_rfc3339(), note_id],
        )?;

        Ok(())
    }

    pub async fn get_orderbook(
        &self,
        base_asset: &str,
        quote_asset: &str,
    ) -> Result<(Vec<SwapNoteRecord>, Vec<SwapNoteRecord>)> {
        let conn = self.conn.lock().unwrap();

        // Get bids (buying base asset with quote asset)
        let mut bid_stmt = conn.prepare(
            "SELECT * FROM swap_notes WHERE status = 'open' AND offered_asset_id = ?1 AND requested_asset_id = ?2 ORDER BY price DESC"
        )?;
        let bid_iter = bid_stmt.query_map(params![quote_asset, base_asset], |row| {
            Ok(Self::row_to_swap_note_record(row).unwrap())
        })?;

        // Get asks (selling base asset for quote asset)
        let mut ask_stmt = conn.prepare(
            "SELECT * FROM swap_notes WHERE status = 'open' AND offered_asset_id = ?1 AND requested_asset_id = ?2 ORDER BY price ASC"
        )?;
        let ask_iter = ask_stmt.query_map(params![base_asset, quote_asset], |row| {
            Ok(Self::row_to_swap_note_record(row).unwrap())
        })?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for bid_result in bid_iter {
            bids.push(bid_result?);
        }

        for ask_result in ask_iter {
            asks.push(ask_result?);
        }

        Ok((bids, asks))
    }

    pub async fn get_user_orders(&self, user_id: &str) -> Result<Vec<SwapNoteRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT * FROM swap_notes WHERE creator_id = ?1 ORDER BY created_at DESC")?;
        let row_iter = stmt.query_map(params![user_id], |row| {
            Ok(Self::row_to_swap_note_record(row).unwrap())
        })?;

        let mut records = Vec::new();
        for record_result in row_iter {
            records.push(record_result?);
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

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&query)?;
        let params: Vec<&dyn rusqlite::ToSql> = note_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let row_iter =
            stmt.query_map(
                params.as_slice(),
                |row| Ok(row.get::<_, String>("note_id")?),
            )?;

        let mut existing_ids = Vec::new();
        for id_result in row_iter {
            existing_ids.push(id_result?);
        }

        Ok(existing_ids)
    }

    pub async fn increment_failure_count(&self, note_id: &str) -> Result<i32> {
        self.execute(
            "UPDATE swap_notes SET failure_count = failure_count + 1, updated_at = ?1 WHERE note_id = ?2",
            params![Utc::now().to_rfc3339(), note_id],
        )?;

        // Get the updated failure count
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT failure_count FROM swap_notes WHERE note_id = ?1")?;
        let failure_count: i32 =
            stmt.query_row(params![note_id], |row| Ok(row.get("failure_count")?))?;

        Ok(failure_count)
    }

    pub async fn move_to_failed_table(&self, note_id: &str, failure_reason: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // First, get the swap note record
        let mut stmt = conn.prepare("SELECT * FROM swap_notes WHERE note_id = ?1")?;
        let swap_note = stmt.query_row(params![note_id], |row| {
            Ok((
                row.get::<_, String>("id")?,
                row.get::<_, String>("note_id")?,
                row.get::<_, String>("creator_id")?,
                row.get::<_, String>("offered_asset_id")?,
                row.get::<_, i64>("offered_amount")?,
                row.get::<_, String>("requested_asset_id")?,
                row.get::<_, i64>("requested_amount")?,
                row.get::<_, f64>("price")?,
                row.get::<_, bool>("is_bid")?,
                row.get::<_, String>("note_data")?,
                row.get::<_, i32>("failure_count").unwrap_or(0),
                row.get::<_, String>("created_at")?,
            ))
        })?;

        drop(stmt);

        // Insert into failed_swap_notes table
        self.execute(
            r#"
            INSERT INTO failed_swap_notes (
                id, note_id, creator_id, offered_asset_id, offered_amount,
                requested_asset_id, requested_amount, price, is_bid,
                note_data, failure_count, failure_reason, created_at, failed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            params![
                swap_note.0,  // id
                swap_note.1,  // note_id
                swap_note.2,  // creator_id
                swap_note.3,  // offered_asset_id
                swap_note.4,  // offered_amount
                swap_note.5,  // requested_asset_id
                swap_note.6,  // requested_amount
                swap_note.7,  // price
                swap_note.8,  // is_bid
                swap_note.9,  // note_data
                swap_note.10, // failure_count
                failure_reason,
                swap_note.11, // created_at
                Utc::now().to_rfc3339()
            ],
        )?;

        // Remove from swap_notes table
        self.execute(
            "DELETE FROM swap_notes WHERE note_id = ?1",
            params![note_id],
        )?;

        Ok(())
    }

    pub async fn get_failed_swap_notes(&self) -> Result<Vec<FailedSwapNoteRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM failed_swap_notes ORDER BY failed_at DESC")?;
        let row_iter = stmt.query_map([], |row| {
            let created_at_str: String = row.get("created_at")?;
            let failed_at_str: String = row.get("failed_at")?;

            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .unwrap()
                .with_timezone(&Utc);
            let failed_at = DateTime::parse_from_rfc3339(&failed_at_str)
                .unwrap()
                .with_timezone(&Utc);

            Ok(FailedSwapNoteRecord {
                id: row.get("id")?,
                note_id: row.get("note_id")?,
                creator_id: row.get("creator_id")?,
                offered_asset_id: row.get("offered_asset_id")?,
                offered_amount: row.get::<_, i64>("offered_amount")? as u64,
                requested_asset_id: row.get("requested_asset_id")?,
                requested_amount: row.get::<_, i64>("requested_amount")? as u64,
                price: row.get("price")?,
                is_bid: row.get("is_bid")?,
                note_data: row.get("note_data")?,
                failure_count: row.get("failure_count")?,
                failure_reason: row.get("failure_reason")?,
                created_at,
                failed_at,
            })
        })?;

        let mut records = Vec::new();
        for record_result in row_iter {
            records.push(record_result?);
        }

        Ok(records)
    }

    pub async fn insert_filled_order(&self, record: &FilledOrderRecord) -> Result<()> {
        self.execute(
            r#"
            INSERT INTO filled_orders (
                id, note_id, creator_id, offered_asset_id, offered_amount,
                requested_asset_id, requested_amount, price, is_bid,
                note_data, original_status, fill_type, transaction_id, created_at, filled_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            "#,
            params![
                &record.id,
                &record.note_id,
                &record.creator_id,
                &record.offered_asset_id,
                record.offered_amount as i64,
                &record.requested_asset_id,
                record.requested_amount as i64,
                record.price,
                record.is_bid,
                &record.note_data,
                record.original_status.to_string(),
                &record.fill_type,
                &record.transaction_id,
                record.created_at.to_rfc3339(),
                record.filled_at.to_rfc3339()
            ],
        )?;

        Ok(())
    }

    pub async fn move_to_filled_orders(
        &self,
        note_id: &str,
        fill_type: &str,
        transaction_id: Option<String>,
    ) -> Result<()> {
        // First, get the swap note record and release the connection lock immediately
        let swap_note = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT * FROM swap_notes WHERE note_id = ?1")?;
            let result = stmt.query_row(params![note_id], |row| {
                let status_str: String = row.get("status")?;
                let created_at_str: String = row.get("created_at")?;

                Ok((
                    row.get::<_, String>("id")?,
                    row.get::<_, String>("note_id")?,
                    row.get::<_, String>("creator_id")?,
                    row.get::<_, String>("offered_asset_id")?,
                    row.get::<_, i64>("offered_amount")?,
                    row.get::<_, String>("requested_asset_id")?,
                    row.get::<_, i64>("requested_amount")?,
                    row.get::<_, f64>("price")?,
                    row.get::<_, bool>("is_bid")?,
                    row.get::<_, String>("note_data")?,
                    status_str,
                    created_at_str,
                ))
            })?;
            result
        }; // Connection lock is released here

        // Parse the status from the database
        let original_status: SwapNoteStatus = swap_note.10.parse()?;

        // Create a filled order record
        let filled_record = FilledOrderRecord {
            id: Uuid::new_v4().to_string(),
            note_id: swap_note.1,
            creator_id: swap_note.2,
            offered_asset_id: swap_note.3,
            offered_amount: swap_note.4 as u64,
            requested_asset_id: swap_note.5,
            requested_amount: swap_note.6 as u64,
            price: swap_note.7,
            is_bid: swap_note.8,
            note_data: swap_note.9,
            original_status,
            fill_type: fill_type.to_string(),
            transaction_id,
            created_at: DateTime::parse_from_rfc3339(&swap_note.11)?.with_timezone(&Utc),
            filled_at: Utc::now(),
        };

        // Insert into filled_orders table
        self.insert_filled_order(&filled_record).await?;

        // Remove from swap_notes table
        self.execute(
            "DELETE FROM swap_notes WHERE note_id = ?1",
            params![note_id],
        )?;

        Ok(())
    }

    pub async fn get_filled_orders(&self) -> Result<Vec<FilledOrderRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM filled_orders ORDER BY filled_at DESC")?;
        let row_iter = stmt.query_map([], |row| {
            let original_status_str: String = row.get("original_status")?;
            let created_at_str: String = row.get("created_at")?;
            let filled_at_str: String = row.get("filled_at")?;

            let original_status = original_status_str.parse().unwrap();
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .unwrap()
                .with_timezone(&Utc);
            let filled_at = DateTime::parse_from_rfc3339(&filled_at_str)
                .unwrap()
                .with_timezone(&Utc);

            Ok(FilledOrderRecord {
                id: row.get("id")?,
                note_id: row.get("note_id")?,
                creator_id: row.get("creator_id")?,
                offered_asset_id: row.get("offered_asset_id")?,
                offered_amount: row.get::<_, i64>("offered_amount")? as u64,
                requested_asset_id: row.get("requested_asset_id")?,
                requested_amount: row.get::<_, i64>("requested_amount")? as u64,
                price: row.get("price")?,
                is_bid: row.get("is_bid")?,
                note_data: row.get("note_data")?,
                original_status,
                fill_type: row.get("fill_type")?,
                transaction_id: row.get("transaction_id")?,
                created_at,
                filled_at,
            })
        })?;

        let mut records = Vec::new();
        for record_result in row_iter {
            records.push(record_result?);
        }

        Ok(records)
    }

    pub async fn get_user_filled_orders(&self, user_id: &str) -> Result<Vec<FilledOrderRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT * FROM filled_orders WHERE creator_id = ?1 ORDER BY filled_at DESC")?;
        let row_iter = stmt.query_map(params![user_id], |row| {
            let original_status_str: String = row.get("original_status")?;
            let created_at_str: String = row.get("created_at")?;
            let filled_at_str: String = row.get("filled_at")?;

            let original_status = original_status_str.parse().unwrap();
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .unwrap()
                .with_timezone(&Utc);
            let filled_at = DateTime::parse_from_rfc3339(&filled_at_str)
                .unwrap()
                .with_timezone(&Utc);

            Ok(FilledOrderRecord {
                id: row.get("id")?,
                note_id: row.get("note_id")?,
                creator_id: row.get("creator_id")?,
                offered_asset_id: row.get("offered_asset_id")?,
                offered_amount: row.get::<_, i64>("offered_amount")? as u64,
                requested_asset_id: row.get("requested_asset_id")?,
                requested_amount: row.get::<_, i64>("requested_amount")? as u64,
                price: row.get("price")?,
                is_bid: row.get("is_bid")?,
                note_data: row.get("note_data")?,
                original_status,
                fill_type: row.get("fill_type")?,
                transaction_id: row.get("transaction_id")?,
                created_at,
                filled_at,
            })
        })?;

        let mut records = Vec::new();
        for record_result in row_iter {
            records.push(record_result?);
        }

        Ok(records)
    }

    /// Force SQLite to checkpoint WAL file and sync to disk
    /// This ensures all pending database changes are immediately visible to subsequent reads
    pub async fn force_sync(&self) -> Result<()> {
        // Force WAL checkpoint to ensure all changes are written to main database file
        self.execute_pragma("PRAGMA wal_checkpoint(TRUNCATE)")?;

        // Force synchronous write to disk
        self.execute_pragma("PRAGMA synchronous = FULL")?;

        Ok(())
    }
}
