use chrono::Local;
use miden_client::{Felt, account::AccountId};
use miden_clob::{
    common::generate_depth_chart, database::Database, note_serialization::deserialize_note,
};
use std::env;
use tokio::{
    self,
    time::{Duration, sleep},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables
    dotenv::dotenv().ok();

    let usdc_faucet_id =
        env::var("USDC_FAUCET_ID").expect("USDC_FAUCET_ID must be set in .env file");
    let eth_faucet_id = env::var("ETH_FAUCET_ID").expect("ETH_FAUCET_ID must be set in .env file");

    // Parse faucet IDs
    let usdc_faucet = AccountId::from_hex(&usdc_faucet_id)?;
    let eth_faucet = AccountId::from_hex(&eth_faucet_id)?;

    // Connect to database
    let database_url = "sqlite:./clob.sqlite3";
    let database = Database::new(database_url).await?;

    println!("🚀 Starting Real-Time Depth Chart Monitor");
    println!("📊 Refreshing every 2 seconds... Press Ctrl+C to exit\n");

    // Main loop for real-time updates
    loop {
        // Clear the terminal screen
        print!("\x1B[2J\x1B[1;1H");

        // Display header with timestamp
        let now = Local::now();
        println!(
            "╔══════════════════════════════════════════════════════════════════════════════════════════════════════════╗"
        );
        println!(
            "║                                    MIDEN CLOB - REAL-TIME DEPTH CHART                                   ║"
        );
        println!(
            "║                                   Last Updated: {}                                   ║",
            now.format("%Y-%m-%d %H:%M:%S")
        );
        println!(
            "╚══════════════════════════════════════════════════════════════════════════════════════════════════════════╝"
        );
        println!();

        match display_depth_chart(&database, &usdc_faucet, &eth_faucet).await {
            Ok(order_count) => {
                if order_count == 0 {
                    println!("📭 No open orders found in the database.");
                    println!("💡 Run the populate script first: cargo run --bin populate");
                } else {
                    println!("\n💡 Tips:");
                    println!("   • Run matching engine: cargo run --bin matching_engine");
                    println!("   • Add more orders: cargo run --bin populate");
                    println!("   • Press Ctrl+C to exit this monitor");
                }
            }
            Err(e) => {
                println!("❌ Error fetching data: {}", e);
                println!("🔄 Will retry in 2 seconds...");
            }
        }

        // println!("\n🔄 Refreshing in 1 seconds...");

        // Wait 2 seconds before next update
        sleep(Duration::from_secs(1)).await;
    }
}

async fn display_depth_chart(
    database: &Database,
    usdc_faucet: &AccountId,
    eth_faucet: &AccountId,
) -> Result<usize, Box<dyn std::error::Error>> {
    // Get all open swap notes from database
    let open_orders = database.get_open_swap_notes().await?;

    if open_orders.is_empty() {
        return Ok(0);
    }

    println!("📊 Found {} open orders\n", open_orders.len());

    // Display open orders table
    println!(
        "╔══════════════════════════════════════════════════════════════════════════════════════════════════════════╗"
    );
    println!(
        "║                                             OPEN ORDERS                                                  ║"
    );
    println!(
        "╠════════════════════════════════════╦══════════════════╦══════════════════╦═══════════╦═══════════════════╣"
    );
    println!(
        "║             Note ID                ║   Offered Asset  ║ Requested Asset  ║   Type    ║     Creator       ║"
    );
    println!(
        "╠════════════════════════════════════╬══════════════════╬══════════════════╬═══════════╬═══════════════════╣"
    );

    let mut swap_notes = Vec::new();
    let mut account_names = Vec::new();
    let mut trader_counter = 1;

    for order in &open_orders {
        // Deserialize the note from base64
        let note = match deserialize_note(&order.note_data) {
            Ok(note) => note,
            Err(e) => {
                println!("⚠️  Failed to deserialize note {}: {}", order.note_id, e);
                continue;
            }
        };

        // Extract offered and requested assets from the note
        let offered_asset = note
            .assets()
            .iter()
            .next()
            .expect("note has no assets")
            .unwrap_fungible();
        let requested_asset_word: [Felt; 4] = [
            note.inputs().values()[0],
            note.inputs().values()[1],
            note.inputs().values()[2],
            note.inputs().values()[3],
        ];
        let requested_asset_id =
            AccountId::try_from([requested_asset_word[3], requested_asset_word[2]])?;
        let requested_amount = requested_asset_word[0].as_int();

        // Determine order type (bid or ask)
        let order_type = if offered_asset.faucet_id() == *usdc_faucet {
            "BID (Buy)"
        } else {
            "ASK (Sell)"
        };

        // Get creator ID
        let creator_prefix = note.inputs().values()[12];
        let creator_suffix = note.inputs().values()[13];
        let creator_id = AccountId::try_from([creator_prefix, creator_suffix])?;

        // Create a simple name for the trader
        let trader_name = format!("Trader{}", trader_counter);
        account_names.push((creator_id, trader_name.clone()));
        trader_counter += 1;

        println!(
            "║ {:<34} ║ {:<16} ║ {:<16} ║ {:<9} ║ {:<17} ║",
            format!("{:.8}...", order.note_id),
            format!(
                "{} {}",
                offered_asset.amount(),
                if offered_asset.faucet_id() == *usdc_faucet {
                    "USDC"
                } else {
                    "ETH"
                }
            ),
            format!(
                "{} {}",
                requested_amount,
                if requested_asset_id == *usdc_faucet {
                    "USDC"
                } else {
                    "ETH"
                }
            ),
            order_type,
            trader_name
        );

        swap_notes.push(note);
    }

    println!(
        "╚════════════════════════════════════╩══════════════════╩══════════════════╩═══════════╩═══════════════════╝"
    );

    // Convert account_names to the format expected by generate_depth_chart
    let account_name_refs: Vec<(AccountId, &str)> = account_names
        .iter()
        .map(|(id, name)| (*id, name.as_str()))
        .collect();

    // Generate and display the depth chart
    generate_depth_chart(&swap_notes, usdc_faucet, eth_faucet, &account_name_refs);

    Ok(open_orders.len())
}
