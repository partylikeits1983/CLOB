use chrono::Local;
use miden_client::{Felt, account::AccountId};
use miden_clob::{
    common::generate_depth_chart, database::Database, note_serialization::deserialize_note,
};
use std::{env, io::{self, Write}};
use tokio::{
    self,
    time::{Duration, sleep},
};
use clap::Parser;

#[derive(Parser)]
#[command(name = "depth_chart")]
#[command(about = "Real-time depth chart monitor for MIDEN CLOB")]
struct Cli {
    /// Show detailed open orders table
    #[arg(long, help = "Display all open orders in a detailed table")]
    show_orders: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let cli = Cli::parse();
    
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

    // Initialize terminal
    initialize_terminal()?;

    println!("🚀 Starting Real-Time Depth Chart Monitor");
    if cli.show_orders {
        println!("📊 Showing detailed open orders table and depth chart");
    } else {
        println!("📊 Showing depth chart and statistics only");
    }
    println!("🔄 Refreshing every 2 seconds... Press Ctrl+C to exit\n");

    // Wait a moment for user to read the startup message
    sleep(Duration::from_millis(200)).await;

    // Main loop for real-time updates
    loop {
        // Clear and prepare the display buffer
        let mut display_buffer = String::new();
        
        // Display header with timestamp
        let now = Local::now();
        display_buffer.push_str(&format!(
            "╔══════════════════════════════════════════════════════════════════════════════════════════════════════════╗\n"
        ));
        display_buffer.push_str(&format!(
            "║                                    MIDEN CLOB - REAL-TIME DEPTH CHART                                   ║\n"
        ));
        display_buffer.push_str(&format!(
            "║                                   Last Updated: {}                                   ║\n",
            now.format("%Y-%m-%d %H:%M:%S")
        ));
        display_buffer.push_str(&format!(
            "╚══════════════════════════════════════════════════════════════════════════════════════════════════════════╝\n"
        ));
        display_buffer.push('\n');

        match display_depth_chart(&database, &usdc_faucet, &eth_faucet, cli.show_orders).await {
            Ok((order_count, chart_content)) => {
                display_buffer.push_str(&chart_content);
                
                if order_count == 0 {
                    display_buffer.push_str("📭 No open orders found in the database.\n");
                    display_buffer.push_str("💡 Run the populate script first: cargo run --bin populate\n");
                } else {
                    display_buffer.push_str("\n💡 Tips:\n");
                    display_buffer.push_str("   • Run matching engine: cargo run --bin matching_engine\n");
                    display_buffer.push_str("   • Add more orders: cargo run --bin populate\n");
                    display_buffer.push_str("   • Press Ctrl+C to exit this monitor\n");
                }
            }
            Err(e) => {
                display_buffer.push_str(&format!("❌ Error fetching data: {}\n", e));
                display_buffer.push_str("🔄 Will retry in 2 seconds...\n");
            }
        }

        // Clear screen and display the buffered content
        clear_screen_smooth()?;
        print!("{}", display_buffer);
        io::stdout().flush()?;

        // Wait 2 seconds before next update
        sleep(Duration::from_millis(20)).await;
    }
}

/// Initialize terminal for smooth updates
fn initialize_terminal() -> io::Result<()> {
    // Hide cursor for cleaner display
    print!("\x1B[?25l");
    io::stdout().flush()?;
    Ok(())
}

/// Clear screen smoothly without flickering
fn clear_screen_smooth() -> io::Result<()> {
    // Move cursor to top-left and clear screen
    print!("\x1B[H\x1B[2J");
    io::stdout().flush()?;
    Ok(())
}

async fn display_depth_chart(
    database: &Database,
    usdc_faucet: &AccountId,
    eth_faucet: &AccountId,
    show_orders: bool,
) -> Result<(usize, String), Box<dyn std::error::Error>> {
    let mut output = String::new();
    
    // Get all open swap notes from database
    let open_orders = database.get_open_swap_notes().await?;

    if open_orders.is_empty() {
        return Ok((0, output));
    }

    if show_orders {
        output.push_str(&format!("📊 Found {} open orders\n\n", open_orders.len()));

        // Display open orders table
        output.push_str("╔══════════════════════════════════════════════════════════════════════════════════════════════════════════╗\n");
        output.push_str("║                                             OPEN ORDERS                                                  ║\n");
        output.push_str("╠════════════════════════════════════╦══════════════════╦══════════════════╦═══════════╦═══════════════════╣\n");
        output.push_str("║             Note ID                ║   Offered Asset  ║ Requested Asset  ║   Type    ║     Creator       ║\n");
        output.push_str("╠════════════════════════════════════╬══════════════════╬══════════════════╬═══════════╬═══════════════════╣\n");
    } else {
        output.push_str(&format!("📊 Processing {} orders for depth chart\n\n", open_orders.len()));
    }

    let mut swap_notes = Vec::new();
    let mut account_names = Vec::new();
    let mut trader_counter = 1;

    for order in &open_orders {
        // Deserialize the note from base64
        let note = match deserialize_note(&order.note_data) {
            Ok(note) => note,
            Err(e) => {
                output.push_str(&format!("⚠️  Failed to deserialize note {}: {}\n", order.note_id, e));
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

        // Determine order type (bid or ask) - only needed for table display
        let order_type = if show_orders {
            if offered_asset.faucet_id() == *usdc_faucet {
                "BID (Buy)"
            } else {
                "ASK (Sell)"
            }
        } else {
            ""
        };

        // Get creator ID
        let creator_prefix = note.inputs().values()[12];
        let creator_suffix = note.inputs().values()[13];
        let creator_id = AccountId::try_from([creator_prefix, creator_suffix])?;

        // Create a simple name for the trader
        let trader_name = format!("Trader{}", trader_counter);
        account_names.push((creator_id, trader_name.clone()));
        trader_counter += 1;

        // Display order row only if show_orders is true
        if show_orders {
            output.push_str(&format!(
                "║ {:<34} ║ {:<16} ║ {:<16} ║ {:<9} ║ {:<17} ║\n",
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
            ));
        }

        swap_notes.push(note);
    }

    // Close the table if it was displayed
    if show_orders {
        output.push_str("╚════════════════════════════════════╩══════════════════╩══════════════════╩═══════════╩═══════════════════╝\n");
    }

    // Convert account_names to the format expected by generate_depth_chart
    let account_name_refs: Vec<(AccountId, &str)> = account_names
        .iter()
        .map(|(id, name)| (*id, name.as_str()))
        .collect();

    // Generate the depth chart and capture its output
    let chart_output = generate_depth_chart_to_string(&swap_notes, usdc_faucet, eth_faucet, &account_name_refs, show_orders);
    output.push_str(&chart_output);

    Ok((open_orders.len(), output))
}

/// Helper function to capture the depth chart output as a string instead of printing
fn generate_depth_chart_to_string(
    swap_notes: &[miden_client::note::Note],
    usdc_faucet: &AccountId,
    eth_faucet: &AccountId,
    account_names: &[(AccountId, &str)],
    show_orders: bool,
) -> String {
    // Use the new string-based function
    miden_clob::common::generate_depth_chart_string(swap_notes, usdc_faucet, eth_faucet, account_names, show_orders)
}
