use reqwest;
use serde_json;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let base_url = "http://localhost:3000";

    println!("🚀 Miden CLOB API Example");
    println!("============================\n");

    // 1. Check server health
    println!("1. Checking server health...");
    let health_response = client.get(&format!("{}/health", base_url)).send().await?;

    if health_response.status().is_success() {
        let health_data: serde_json::Value = health_response.json().await?;
        println!("✅ Server is healthy: {}", health_data);
    } else {
        println!("❌ Server health check failed");
        return Ok(());
    }

    sleep(Duration::from_secs(1)).await;

    // 2. Get server statistics
    println!("\n2. Getting server statistics...");
    let stats_response = client.get(&format!("{}/stats", base_url)).send().await?;

    if stats_response.status().is_success() {
        let stats_data: serde_json::Value = stats_response.json().await?;
        println!(
            "📊 Server stats: {}",
            serde_json::to_string_pretty(&stats_data)?
        );
    }

    sleep(Duration::from_secs(1)).await;

    // 3. Try to get orderbook (this will likely be empty initially)
    println!("\n3. Getting orderbook...");
    let orderbook_response = client
        .get(&format!(
            "{}/orderbook?base_asset=eth_faucet_id&quote_asset=usdc_faucet_id",
            base_url
        ))
        .send()
        .await?;

    if orderbook_response.status().is_success() {
        let orderbook_data: serde_json::Value = orderbook_response.json().await?;
        println!(
            "📈 Orderbook: {}",
            serde_json::to_string_pretty(&orderbook_data)?
        );
    } else {
        println!("⚠️  Could not get orderbook (might be empty or invalid asset IDs)");
    }

    sleep(Duration::from_secs(1)).await;

    // 4. Trigger matching (this is safe to call even with no orders)
    println!("\n4. Triggering matching cycle...");
    let match_response = client.post(&format!("{}/match", base_url)).send().await?;

    if match_response.status().is_success() {
        let match_data: serde_json::Value = match_response.json().await?;
        println!(
            "🔄 Matching result: {}",
            serde_json::to_string_pretty(&match_data)?
        );
    } else {
        println!("⚠️  Matching request failed");
    }

    sleep(Duration::from_secs(1)).await;

    // 5. Example of submitting an order (this will fail without proper note data)
    println!("\n5. Example order submission (will fail - demo only)...");
    let order_payload = serde_json::json!({
        "note_data": "fake_base64_encoded_note_data"
    });

    let submit_response = client
        .post(&format!("{}/orders/submit", base_url))
        .json(&order_payload)
        .send()
        .await?;

    let submit_data: serde_json::Value = submit_response.json().await?;
    println!(
        "📝 Order submission result: {}",
        serde_json::to_string_pretty(&submit_data)?
    );

    println!("\n✨ API Example completed!");
    println!("\n💡 Next steps:");
    println!("   1. Run the populate script to add real orders:");
    println!("      cargo run --bin populate -- --once");
    println!("   2. Try the API calls again to see populated data");
    println!("   3. Build a frontend that uses these endpoints");

    Ok(())
}
