mod blockchain_executor;
mod common;
mod database;
mod note_serialization;
mod orderbook;
mod server;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber;

use database::Database;
use orderbook::OrderBookManager;
use server::{AppState, create_router};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("info,miden_clob=debug")
        .init();

    info!("Starting Miden CLOB Server");

    // Initialize database with proper write permissions
    let database_url = "sqlite:./clob.sqlite3?mode=rwc";

    // Ensure the database file has write permissions
    if std::path::Path::new("./clob.sqlite3").exists() {
        use std::fs;
        let metadata = fs::metadata("./clob.sqlite3")?;
        if metadata.permissions().readonly() {
            let mut perms = metadata.permissions();
            perms.set_readonly(false);
            fs::set_permissions("./clob.sqlite3", perms)?;
            info!("Fixed database file permissions");
        }
    }

    let db = Arc::new(Database::new(database_url).await?);
    info!("Database initialized with write permissions");

    // Initialize orderbook manager
    let mut orderbook_manager = OrderBookManager::new();
    orderbook_manager.initialize_from_database(&db).await?;
    let orderbook_manager = Arc::new(RwLock::new(orderbook_manager));
    info!("Orderbook manager initialized");

    // Create application state
    let state = AppState {
        db,
        orderbook_manager,
    };

    // Create router
    let app = create_router(state);

    // Bind to address
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    info!("Server listening on http://0.0.0.0:3000");

    // Print available endpoints
    println!("\n🚀 Miden CLOB Server is running!");
    println!("📡 Available endpoints:");
    println!("  GET  /health                    - Health check");
    println!("  POST /orders/submit             - Submit a new order");
    println!("  GET  /orderbook?base=X&quote=Y  - Get orderbook");
    println!("  GET  /orders/user?user_id=X     - Get user orders");
    println!("  GET  /depth/:base/:quote        - Get depth chart");
    println!("  POST /match                     - Trigger matching");
    println!("  GET  /stats                     - Get server stats");
    println!("🌐 Server address: http://localhost:3000");
    println!("📊 Example: http://localhost:3000/health\n");

    // Start server
    axum::serve(listener, app).await?;

    Ok(())
}
