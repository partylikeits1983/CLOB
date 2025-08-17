mod common;
mod database;
mod note_serialization;
mod orderbook;
mod server;

use anyhow::Result;
use dotenv::dotenv;
use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber;

use database::Database;
use orderbook::OrderBookManager;
use server::{create_router, AppState};

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    dotenv().ok();

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

    // Get port from environment variable
    let server_url = env::var("SERVER_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let port = server_url.split(':').last().unwrap_or("8080");
    let bind_address = format!("0.0.0.0:{}", port);

    // Bind to address
    let listener = tokio::net::TcpListener::bind(&bind_address).await?;
    info!("Server listening on http://{}", bind_address);

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
    println!("🌐 Server address: {}", server_url);
    println!("📊 Example: {}/health\n", server_url);

    // Start server
    axum::serve(listener, app).await?;

    Ok(())
}
