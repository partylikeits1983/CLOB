mod common;
mod database;
mod note_serialization;
mod orderbook;
mod server;

use anyhow::{Result, anyhow};
use dotenv::dotenv;
use rand::{Rng, rng};
use reqwest;
use serde::Deserialize;
use std::env;
use std::fs;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::common::{
    delete_keystore_and_store, instantiate_client, price_to_swap_note, setup_accounts_and_faucets,
};
use miden_client::{
    Client,
    account::{Account, AccountId},
    keystore::FilesystemKeyStore,
    rpc::Endpoint,
    transaction::{OutputNote, TransactionRequestBuilder},
};
use miden_crypto::rand::FeltRng;

#[derive(Debug, Deserialize)]
struct CoinGeckoResponse {
    ethereum: PriceData,
}

#[derive(Debug, Deserialize)]
struct PriceData {
    usd: f64,
}

#[derive(Debug, Clone)]
struct MarketMakerConfig {
    pub spread_percentage: f64, // Spread around mid price (e.g., 0.5 for 0.5%)
    pub num_levels: usize,      // Number of price levels per side
    pub base_quantity: f64,     // Base quantity in ETH
    pub quantity_variance: f64, // Variance in quantities (e.g., 0.3 for ±30%)
    pub price_variance: f64,    // Variance in price levels (e.g., 0.1 for ±10%)
    pub update_interval_secs: u64, // How often to refresh orders
}

impl Default for MarketMakerConfig {
    fn default() -> Self {
        Self {
            spread_percentage: 0.5,
            num_levels: 5,
            base_quantity: 1.0,
            quantity_variance: 0.3,
            price_variance: 0.1,
            update_interval_secs: 60,
        }
    }
}

struct MarketMaker {
    client: Option<Client>,
    accounts: Vec<Account>,
    config: MarketMakerConfig,
    server_url: String,
    http_client: reqwest::Client,
    is_setup_mode: bool,
}

impl MarketMaker {
    async fn new(
        config: MarketMakerConfig,
        server_url: String,
        is_setup_mode: bool,
    ) -> Result<Self> {
        info!("Initializing Market Maker");

        let http_client = reqwest::Client::new();

        if is_setup_mode {
            info!("Running in setup mode - will create faucets and accounts");

            // Load environment variables first
            dotenv().ok();

            // Clean previous data
            delete_keystore_and_store().await;

            // Initialize Miden client
            let endpoint: Endpoint =
                Endpoint::try_from(env::var("MIDEN_NODE_ENDPOINT").unwrap().as_str()).unwrap();

            let mut client = instantiate_client(endpoint).await?;
            client.sync_state().await.unwrap();

            let keystore = FilesystemKeyStore::new("./keystore".into())?;

            // Setup accounts with balances for market making + matcher account
            let balances = vec![
                vec![50_000, 50],       // Market maker 1
                vec![40_000, 40],       // Market maker 2
                vec![30_000, 30],       // Market maker 3
                vec![20_000, 20],       // Market maker 4
                vec![10_000_000, 1000], // Matcher account with large balances
            ];

            let (accounts, faucets) = setup_accounts_and_faucets(
                &mut client,
                keystore,
                5, // 4 market maker accounts + 1 matcher account
                2, // USDC and ETH faucets
                balances,
            )
            .await?;

            // The last account is the matcher
            let matcher_account = accounts.last().unwrap();

            info!(
                "Created {} market maker accounts + 1 matcher account with {} faucets",
                accounts.len() - 1,
                faucets.len()
            );
            info!("Matcher account ID: {}", matcher_account.id().to_hex());

            // Save faucet IDs and matcher account to .env file
            let miden_endpoint = env::var("MIDEN_NODE_ENDPOINT").unwrap();
            let env_content = format!(
                "# Miden CLOB Configuration\n\
                # Faucet IDs for the trading pair\n\
                USDC_FAUCET_ID={}\n\
                ETH_FAUCET_ID={}\n\
                \n\
                # Matcher account ID\n\
                MATCHER_ACCOUNT_ID={}\n\
                \n\
                # Miden Node Endpoint\n\
                MIDEN_NODE_ENDPOINT={}\n\
                \n\
                # Server configuration\n\
                SERVER_URL={}\n\
                \n\
                # Market maker configuration\n\
                SPREAD_PERCENTAGE={}\n\
                NUM_LEVELS={}\n\
                BASE_QUANTITY={}\n\
                QUANTITY_VARIANCE={}\n\
                PRICE_VARIANCE={}\n\
                UPDATE_INTERVAL_SECS={}\n",
                faucets[0].id().to_hex(),      // USDC
                faucets[1].id().to_hex(),      // ETH
                matcher_account.id().to_hex(), // Matcher account
                miden_endpoint,                // Miden node endpoint
                server_url,
                config.spread_percentage,
                config.num_levels,
                config.base_quantity,
                config.quantity_variance,
                config.price_variance,
                config.update_interval_secs,
            );

            fs::write(".env", env_content)?;
            info!("Saved faucet IDs and matcher account to .env file");

            Ok(Self {
                client: Some(client),
                accounts,
                config,
                server_url,
                http_client,
                is_setup_mode: true,
            })
        } else {
            // Load from .env file for normal operation
            dotenv().ok();

            let usdc_faucet_id = env::var("USDC_FAUCET_ID").map_err(|_| {
                anyhow!("USDC_FAUCET_ID not found in .env file. Run with --setup first.")
            })?;
            let eth_faucet_id = env::var("ETH_FAUCET_ID").map_err(|_| {
                anyhow!("ETH_FAUCET_ID not found in .env file. Run with --setup first.")
            })?;

            if usdc_faucet_id.is_empty() || eth_faucet_id.is_empty() {
                return Err(anyhow!(
                    "Faucet IDs are empty in .env file. Run with --setup first."
                ));
            }

            // Initialize client and import faucets
            let endpoint: Endpoint =
                Endpoint::try_from(env::var("MIDEN_NODE_ENDPOINT").unwrap().as_str()).unwrap();

            let mut client = instantiate_client(endpoint).await?;
            client.sync_state().await.unwrap();

            let usdc_id = AccountId::from_hex(&usdc_faucet_id)?;
            let eth_id = AccountId::from_hex(&eth_faucet_id)?;

            // Try to import faucets, but don't fail if they don't exist
            if let Err(e) = client.import_account_by_id(usdc_id).await {
                warn!(
                    "Failed to import USDC faucet (this is expected if running without setup): {}",
                    e
                );
            }
            if let Err(e) = client.import_account_by_id(eth_id).await {
                warn!(
                    "Failed to import ETH faucet (this is expected if running without setup): {}",
                    e
                );
            }

            info!(
                "Attempted to import faucets: USDC={}, ETH={}",
                usdc_faucet_id, eth_faucet_id
            );

            Ok(Self {
                client: Some(client),
                accounts: Vec::new(), // Will be loaded if needed
                config,
                server_url,
                http_client,
                is_setup_mode: false,
            })
        }
    }

    async fn fetch_eth_price(&self) -> Result<f64> {
        let url = "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=usd";

        info!("Fetching ETH price from CoinGecko");
        let response = self.http_client.get(url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch price: HTTP {}", response.status()));
        }

        let price_data: CoinGeckoResponse = response.json().await?;
        let eth_price = price_data.ethereum.usd;

        info!("Current ETH price: ${:.2}", eth_price);
        Ok(eth_price)
    }

    async fn run_single_cycle(&mut self) -> Result<()> {
        if self.is_setup_mode {
            info!("Setup mode completed successfully");
            return Ok(());
        }

        info!("Running market making cycle with real SWAP notes");

        // Fetch current ETH price
        let eth_price = self.fetch_eth_price().await?;
        info!("Current ETH price: ${:.2}", eth_price);

        // Generate and submit real SWAP notes
        if self.client.is_some() {
            if let Err(e) = self.generate_and_submit_real_orders(eth_price).await {
                warn!("Failed to generate orders: {}", e);
                return Err(e);
            }
        } else {
            warn!("No client available for order generation");
        }

        // Trigger server matching
        if let Err(e) = self.trigger_server_matching().await {
            warn!("Failed to trigger server matching: {}", e);
        }

        // Check server health and stats
        self.check_server_status().await?;

        info!(
            "Market making cycle completed - ETH price: ${:.2}",
            eth_price
        );
        println!("📊 Current ETH price: ${:.2}", eth_price);
        println!("🔗 Server API working at: {}", self.server_url);

        Ok(())
    }

    async fn generate_and_submit_real_orders(&mut self, eth_price: f64) -> Result<()> {
        info!(
            "Generating real SWAP notes around ETH price ${:.2}",
            eth_price
        );

        // Load environment variables
        dotenv().ok();
        let usdc_faucet_id = AccountId::from_hex(&env::var("USDC_FAUCET_ID")?)?;
        let eth_faucet_id = AccountId::from_hex(&env::var("ETH_FAUCET_ID")?)?;

        let mut rng = rng();
        let spread = eth_price * self.config.spread_percentage / 100.0;
        let half_spread = spread / 2.0;

        let mut orders = Vec::new();

        // Generate bid orders (buying ETH with USDC) - some will overlap with asks
        for level in 0..self.config.num_levels {
            let price_offset = (level + 1) as f64 * (half_spread / self.config.num_levels as f64);
            let price_variance =
                rng.random_range(-self.config.price_variance..self.config.price_variance);

            // Create overlapping orders: some bids will be higher than market price
            let overlap_factor = if level == 0 {
                // First bid crosses into ask territory to create matches
                rng.random_range(0.5..1.5)
            } else if level == 1 {
                // Second bid might cross
                rng.random_range(0.0..1.2)
            } else {
                // Later bids stay below market
                rng.random_range(-0.5..0.5)
            };

            let bid_price = (eth_price - half_spread - price_offset
                + (half_spread * overlap_factor))
                * (1.0 + price_variance);

            let quantity_variance =
                rng.random_range(-self.config.quantity_variance..self.config.quantity_variance);
            let eth_quantity = self.config.base_quantity * (1.0 + quantity_variance);
            let eth_quantity = eth_quantity.max(0.01).min(10.0); // Clamp between 0.01 and 10 ETH

            // Use existing accounts for orders - cycling through available accounts
            let creator_account = if !self.accounts.is_empty() {
                self.accounts[level % self.accounts.len()].id()
            } else {
                // Fallback: use faucet ID if no accounts available
                usdc_faucet_id
            };

            let last_filler_account = creator_account; // Same account initially

            let serial_num = if let Some(ref mut client) = self.client {
                client.rng().draw_word()
            } else {
                return Err(anyhow!(
                    "Client not available for generating random serial numbers"
                ));
            };

            // Create bid order (buying ETH with USDC)
            let swap_note = price_to_swap_note(
                creator_account,
                last_filler_account,
                true, // BID: buying ETH with USDC
                bid_price as u64,
                (eth_quantity * 1000.0) as u64, // Convert to smaller units
                &eth_faucet_id,
                &usdc_faucet_id,
                serial_num,
            );

            // Submit the note as a transaction to the blockchain
            if let Some(ref mut client) = self.client {
                client.sync_state().await.unwrap();

                let req = TransactionRequestBuilder::new()
                    .with_own_output_notes(vec![OutputNote::Full(swap_note.clone())])
                    .build()?;
                let tx = client.new_transaction(creator_account, req).await?;
                client.submit_transaction(tx).await?;

                info!(
                    "✅ Submitted BID transaction: {:.4} ETH @ ${:.2}",
                    eth_quantity, bid_price
                );
            }

            orders.push(swap_note);
        }

        // Generate ask orders (selling ETH for USDC) - some will overlap with bids
        for level in 0..self.config.num_levels {
            let price_offset = (level + 1) as f64 * (half_spread / self.config.num_levels as f64);
            let price_variance =
                rng.random_range(-self.config.price_variance..self.config.price_variance);

            // Create overlapping orders: some asks will be lower than market price
            let overlap_factor = if level == 0 {
                // First ask crosses into bid territory to create matches
                rng.random_range(0.5..1.5)
            } else if level == 1 {
                // Second ask might cross
                rng.random_range(0.0..1.2)
            } else {
                // Later asks stay above market
                rng.random_range(-0.5..0.5)
            };

            let ask_price = (eth_price + half_spread + price_offset
                - (half_spread * overlap_factor))
                * (1.0 + price_variance);

            let quantity_variance =
                rng.random_range(-self.config.quantity_variance..self.config.quantity_variance);
            let eth_quantity = self.config.base_quantity * (1.0 + quantity_variance);
            let eth_quantity = eth_quantity.max(0.01).min(10.0); // Clamp between 0.01 and 10 ETH

            // Use existing accounts for orders - cycling through available accounts
            let creator_account = if !self.accounts.is_empty() {
                self.accounts[level % self.accounts.len()].id()
            } else {
                // Fallback: use faucet ID if no accounts available
                eth_faucet_id
            };

            let last_filler_account = creator_account; // Same account initially

            let serial_num = if let Some(ref mut client) = self.client {
                client.rng().draw_word()
            } else {
                return Err(anyhow!(
                    "Client not available for generating random serial numbers"
                ));
            };

            // Create ask order (selling ETH for USDC)
            let swap_note = price_to_swap_note(
                creator_account,
                last_filler_account,
                false, // ASK: selling ETH for USDC
                ask_price as u64,
                (eth_quantity * 1000.0) as u64, // Convert to smaller units
                &eth_faucet_id,
                &usdc_faucet_id,
                serial_num,
            );

            // Submit the note as a transaction to the blockchain
            if let Some(ref mut client) = self.client {
                client.sync_state().await.unwrap();

                let req = TransactionRequestBuilder::new()
                    .with_own_output_notes(vec![OutputNote::Full(swap_note.clone())])
                    .build()?;
                let tx = client.new_transaction(creator_account, req).await?;
                client.submit_transaction(tx).await?;
                info!(
                    "✅ Submitted ASK transaction: {:.4} ETH @ ${:.2}",
                    eth_quantity, ask_price
                );
            }

            orders.push(swap_note);
        }

        // Submit orders to server
        for (i, note) in orders.iter().enumerate() {
            let note_data = crate::note_serialization::serialize_note(note)?;

            let submit_request = serde_json::json!({
                "note_data": note_data
            });

            let response = self
                .http_client
                .post(&format!("{}/orders/submit", self.server_url))
                .json(&submit_request)
                .send()
                .await?;

            if response.status().is_success() {
                info!(
                    "Successfully submitted order {}/{} to server",
                    i + 1,
                    orders.len()
                );
            } else {
                warn!(
                    "Failed to submit order {}/{}: HTTP {}",
                    i + 1,
                    orders.len(),
                    response.status()
                );
            }

            sleep(Duration::from_millis(100)).await;
        }

        info!("Generated and submitted {} real SWAP orders", orders.len());
        Ok(())
    }

    async fn check_server_status(&self) -> Result<()> {
        info!("Checking server status");

        // Check health endpoint
        match self
            .http_client
            .get(&format!("{}/health", self.server_url))
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() {
                    info!("✅ Server health check passed");
                } else {
                    warn!("⚠️ Server health check failed: HTTP {}", response.status());
                }
            }
            Err(e) => {
                warn!("❌ Failed to connect to server: {}", e);
            }
        }

        // Check stats endpoint
        match self
            .http_client
            .get(&format!("{}/stats", self.server_url))
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() {
                    if let Ok(stats) = response.json::<serde_json::Value>().await {
                        info!("📊 Server stats: {}", stats);
                    }
                } else {
                    warn!("⚠️ Server stats failed: HTTP {}", response.status());
                }
            }
            Err(e) => {
                warn!("❌ Failed to get server stats: {}", e);
            }
        }

        Ok(())
    }

    async fn run_market_making_loop(&mut self) -> Result<()> {
        info!("Starting demo market making loop");

        loop {
            match self.run_single_cycle().await {
                Ok(_) => {
                    info!("Demo market making cycle completed successfully");
                }
                Err(e) => {
                    error!("Error in demo market making cycle: {}", e);
                }
            }

            info!(
                "Waiting {} seconds before next cycle",
                self.config.update_interval_secs
            );
            sleep(Duration::from_secs(self.config.update_interval_secs)).await;
        }
    }

    async fn trigger_server_matching(&self) -> Result<()> {
        let response = self
            .http_client
            .post(&format!("{}/match", self.server_url))
            .send()
            .await?;

        if response.status().is_success() {
            info!("Successfully triggered server matching");
        } else {
            warn!(
                "Failed to trigger server matching: HTTP {}",
                response.status()
            );
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("info,miden_clob=debug")
        .init();

    info!("Starting Miden CLOB Market Maker");

    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    let is_setup_mode = args.len() > 1 && args[1] == "--setup";
    let is_once_mode =
        args.len() > 1 && (args[1] == "--once" || (args.len() > 2 && args[2] == "--once"));

    // Configuration
    let config = MarketMakerConfig {
        spread_percentage: 1.0,   // 1% spread
        num_levels: 3,            // 3 levels per side
        base_quantity: 0.5,       // 0.5 ETH base size
        quantity_variance: 0.4,   // ±40% quantity variance
        price_variance: 0.05,     // ±5% price variance
        update_interval_secs: 30, // Update every 30 seconds
    };

    let server_url = "http://localhost:3000".to_string();

    if is_setup_mode {
        println!("🔧 Running in SETUP mode - creating faucets and accounts");
        println!("This will:");
        println!("  📁 Clear existing keystore and store");
        println!("  🏭 Create USDC and ETH faucets");
        println!("  👤 Create market maker accounts with initial balances");
        println!("  🤖 Create matcher account with 10M USDC and 1000 ETH");
        println!("  💾 Save configuration to .env file");
        println!();
    } else {
        println!("🎯 Running in NORMAL mode - using existing faucets from .env");
        println!("If this fails, run with --setup first to create faucets");
        println!();
    }

    // Create and run market maker
    let mut market_maker = MarketMaker::new(config, server_url, is_setup_mode).await?;

    if is_setup_mode {
        println!("✅ Setup completed successfully!");
        println!("📄 Faucet IDs and matcher account saved to .env file");
        println!("🤖 Matcher account created with 10M USDC and 1000 ETH for matching");
        println!("🚀 Now run without --setup to start market making");
        return Ok(());
    }

    println!("🎯 Market Maker Configuration:");
    println!("  📊 Spread: {:.1}%", market_maker.config.spread_percentage);
    println!("  📈 Levels per side: {}", market_maker.config.num_levels);
    println!(
        "  💰 Base quantity: {:.2} ETH",
        market_maker.config.base_quantity
    );
    println!(
        "  🔄 Update interval: {}s",
        market_maker.config.update_interval_secs
    );
    println!("  🌐 Server URL: {}", market_maker.server_url);
    println!();

    // Check if we should run once or continuously
    if is_once_mode {
        info!("Running single market making cycle");
        market_maker.run_single_cycle().await?;
        println!("✅ Single cycle completed");
    } else {
        info!("Running continuous market making");
        println!("🚀 Starting continuous market making...");
        println!("💡 Tip: Use --once flag to run just one cycle");
        println!("💡 Tip: Use --setup flag to initialize faucets and accounts");
        println!("🛑 Press Ctrl+C to stop");
        market_maker.run_market_making_loop().await?;
    }

    Ok(())
}
