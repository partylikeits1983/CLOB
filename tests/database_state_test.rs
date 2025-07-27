use std::env;
use tokio;

use miden_client::account::AccountId;
use miden_clob::{
    common::{calculate_depth_chart_data, decompose_swapp_note},
    database::Database,
    note_serialization::deserialize_note,
};

#[tokio::test]
async fn test_database_state_and_depth_chart() {
    // Initialize logging for the test
    env_logger::init();

    println!("🔍 Testing database state and depth chart calculation...");

    // Connect to the database
    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:./clob.sqlite3".to_string());
    let db = Database::new(&database_url)
        .await
        .expect("Failed to connect to database");

    // Get all open swap notes from the database
    let open_orders = db
        .get_open_swap_notes()
        .await
        .expect("Failed to get open swap notes");

    println!("📊 Found {} open orders in database", open_orders.len());

    // Print detailed information about each order
    for (i, order) in open_orders.iter().enumerate() {
        println!("\n--- Order {} ---", i + 1);
        println!("ID: {}", order.id);
        println!("Note ID: {}", order.note_id);
        println!("Creator ID: {}", order.creator_id);
        println!("Offered Asset ID: {}", order.offered_asset_id);
        println!(
            "Offered Amount: {} (raw: {})",
            order.offered_amount as f64 / 1e8,
            order.offered_amount
        );
        println!("Requested Asset ID: {}", order.requested_asset_id);
        println!(
            "Requested Amount: {} (raw: {})",
            order.requested_amount as f64 / 1e8,
            order.requested_amount
        );
        println!("Price: {}", order.price);
        println!("Is Bid: {}", order.is_bid);
        println!("Status: {:?}", order.status);
        println!("Created At: {}", order.created_at);

        // Try to deserialize the note data
        match deserialize_note(&order.note_data) {
            Ok(note) => {
                println!("✅ Note deserialized successfully");

                // Try to decompose the swap note
                match decompose_swapp_note(&note) {
                    Ok((offered, requested)) => {
                        println!(
                            "  Offered: {} × {} (faucet: {})",
                            offered.amount(),
                            offered.amount() as f64 / 1e8,
                            offered.faucet_id()
                        );
                        println!(
                            "  Requested: {} × {} (faucet: {})",
                            requested.amount(),
                            requested.amount() as f64 / 1e8,
                            requested.faucet_id()
                        );

                        // Calculate price from note data
                        let note_price = if requested.amount() > 0 {
                            offered.amount() as f64 / requested.amount() as f64
                        } else {
                            0.0
                        };
                        println!("  Calculated price from note: {}", note_price);
                        println!("  Database stored price: {}", order.price);

                        if (note_price - order.price).abs() > 0.001 {
                            println!("  ⚠️  Price mismatch detected!");
                        }
                    }
                    Err(e) => {
                        println!("  ❌ Failed to decompose swap note: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("❌ Failed to deserialize note: {}", e);
            }
        }
    }

    // Test the depth chart calculation using the working logic
    println!("\n🧮 Testing depth chart calculation...");

    // Get faucet IDs from environment variables
    let usdc_faucet = env::var("USDC_FAUCET_ID")
        .ok()
        .and_then(|id| AccountId::from_hex(&id).ok());
    let eth_faucet = env::var("ETH_FAUCET_ID")
        .ok()
        .and_then(|id| AccountId::from_hex(&id).ok());

    println!("USDC Faucet ID: {:?}", usdc_faucet);
    println!("ETH Faucet ID: {:?}", eth_faucet);

    if let (Some(usdc_faucet), Some(eth_faucet)) = (usdc_faucet, eth_faucet) {
        // Convert database records to Notes
        let mut notes = Vec::new();
        for order in &open_orders {
            match deserialize_note(&order.note_data) {
                Ok(note) => notes.push(note),
                Err(e) => println!("Failed to deserialize note {}: {}", order.note_id, e),
            }
        }

        // Calculate depth chart data using the working logic
        let (bids, asks) = calculate_depth_chart_data(&notes, &usdc_faucet, &eth_faucet);

        println!("\n📈 Depth Chart Results:");
        println!("Bids ({}):", bids.len());
        for (i, (price, amount)) in bids.iter().enumerate() {
            println!(
                "  {}: Price: {:.2}, Amount: {:.8} ETH",
                i + 1,
                price,
                amount / 1e8
            );
        }

        println!("\nAsks ({}):", asks.len());
        for (i, (price, amount)) in asks.iter().enumerate() {
            println!(
                "  {}: Price: {:.2}, Amount: {:.8} ETH",
                i + 1,
                price,
                amount / 1e8
            );
        }

        // Calculate spread
        if !bids.is_empty() && !asks.is_empty() {
            let best_bid = bids[0].0;
            let best_ask = asks[0].0;
            let spread = best_ask - best_bid;
            let spread_percentage = (spread / best_bid) * 100.0;
            let mid_price = (best_bid + best_ask) / 2.0;

            println!("\n💰 Market Summary:");
            println!("Best Bid: {:.2} USDC", best_bid);
            println!("Best Ask: {:.2} USDC", best_ask);
            println!("Spread: {:.2} USDC ({:.2}%)", spread, spread_percentage);
            println!("Mid Price: {:.2} USDC", mid_price);
        } else {
            println!("\n⚠️  No spread available - missing bids or asks");
        }
    } else {
        println!("❌ Cannot test depth chart calculation - faucet IDs not available");
        println!("Please set USDC_FAUCET_ID and ETH_FAUCET_ID environment variables");
    }

    println!("\n✅ Database state test completed!");
}
