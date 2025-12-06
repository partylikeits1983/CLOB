use miden_client::{account::AccountId, note::Note};

/// Generates and prints a comprehensive depth chart for USDC/ETH orderbook
pub fn generate_depth_chart(
    swap_notes: &[Note],
    faucet_usdc_id: &AccountId,
    _faucet_eth_id: &AccountId,
    account_names: &[(AccountId, &str)],
) {
    generate_depth_chart_with_options(
        swap_notes,
        faucet_usdc_id,
        _faucet_eth_id,
        account_names,
        true,
    );
}

/// Generates and prints a depth chart with optional detailed orderbook table
pub fn generate_depth_chart_with_options(
    swap_notes: &[Note],
    faucet_usdc_id: &AccountId,
    _faucet_eth_id: &AccountId,
    account_names: &[(AccountId, &str)],
    show_detailed_orderbook: bool,
) {
    let output = generate_depth_chart_string(
        swap_notes,
        faucet_usdc_id,
        _faucet_eth_id,
        account_names,
        show_detailed_orderbook,
    );
    print!("{}", output);
}

/// Generates a depth chart as a string with optional detailed orderbook table
pub fn generate_depth_chart_string(
    swap_notes: &[Note],
    faucet_usdc_id: &AccountId,
    _faucet_eth_id: &AccountId,
    account_names: &[(AccountId, &str)],
    show_detailed_orderbook: bool,
) -> String {
    let mut output = String::new();
    // ANSI color codes
    const GREEN: &str = "\x1b[32m";
    const RED: &str = "\x1b[31m";
    const RESET: &str = "\x1b[0m";
    const BOLD: &str = "\x1b[1m";

    output.push_str("\n╔══════════════════════════════════════════════════════════╗\n");
    output.push_str(&format!(
        "║{}                 USDC/ETH ORDERBOOK DEPTH CHART           {}║\n",
        BOLD, RESET
    ));
    output.push_str("╚══════════════════════════════════════════════════════════╝\n");

    // Separate bids and asks
    let mut bids = Vec::new();
    let mut asks = Vec::new();

    for note in swap_notes {
        if let Ok((offered, requested)) = crate::swap::operations::decompose_swapp_note(note) {
            let creator_id = crate::swap::operations::creator_of(note);
            let creator_name = account_names
                .iter()
                .find(|(id, _)| *id == creator_id)
                .map(|(_, name)| *name)
                .unwrap_or("Unknown");

            // Check if this is a bid (buying ETH with USDC) or ask (selling ETH for USDC)
            if offered.faucet_id() == *faucet_usdc_id {
                // This is a bid (buy order)
                // For bids: price = USDC amount / ETH amount
                let usdc_amount = offered.amount() as f64;
                let eth_amount = requested.amount() as f64;

                // Ensure we don't divide by zero
                if eth_amount > 0.0 {
                    let price = usdc_amount / eth_amount;
                    bids.push((price, eth_amount, creator_name));
                }
            } else {
                // This is an ask (sell order)
                // For asks: price = USDC amount / ETH amount
                let usdc_amount = requested.amount() as f64;
                let eth_amount = offered.amount() as f64;

                // Ensure we don't divide by zero
                if eth_amount > 0.0 {
                    let price = usdc_amount / eth_amount;
                    asks.push((price, eth_amount, creator_name));
                }
            }
        }
    }

    // Sort bids by price (descending)
    bids.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

    // Sort asks by price (ascending)
    asks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    // Calculate cumulative volumes and prepare for visualization
    let mut bid_prices = Vec::new();
    let mut bid_volumes = Vec::new();
    let mut bid_cumulative = Vec::new();
    let mut bid_traders = Vec::new();

    let mut ask_prices = Vec::new();
    let mut ask_volumes = Vec::new();
    let mut ask_cumulative = Vec::new();
    let mut ask_traders = Vec::new();

    let mut cumulative_bid_volume_usd = 0.0;
    for (price, volume, trader) in &bids {
        bid_prices.push(*price);
        let volume_usd = volume * price;
        bid_volumes.push(volume_usd);
        cumulative_bid_volume_usd += volume_usd;
        bid_cumulative.push(cumulative_bid_volume_usd);
        bid_traders.push(trader);
    }

    let mut cumulative_ask_volume_usd = 0.0;
    for (price, volume, trader) in &asks {
        ask_prices.push(*price);
        let volume_usd = volume * price;
        ask_volumes.push(volume_usd);
        cumulative_ask_volume_usd += volume_usd;
        ask_cumulative.push(cumulative_ask_volume_usd);
        ask_traders.push(trader);
    }

    // Find the maximum cumulative volume for scaling
    let max_cumulative = f64::max(
        bid_cumulative.last().copied().unwrap_or(0.0),
        ask_cumulative.last().copied().unwrap_or(0.0),
    );

    // Calculate the spread
    let spread_info = if !bids.is_empty() && !asks.is_empty() {
        let best_bid = bids[0].0;
        let best_ask = asks[0].0;
        let spread = best_ask - best_bid;
        let spread_percentage = (spread / best_bid) * 100.0;
        let mid_price = (best_bid + best_ask) / 2.0;

        format!(
            "Best Bid: {:.2} USDC | Best Ask: {:.2} USDC | Spread: {:.2} ({:.2}%) | Mid: {:.2}",
            best_bid, best_ask, spread, spread_percentage, mid_price
        )
    } else {
        "No spread available - missing bids or asks".to_string()
    };

    // Print market summary
    output.push_str(
        "\n════════════════════════════════════════════════════════════════════════════════════════════╗\n"
    );
    output.push_str(
        "║                                MARKET SUMMARY                                             ║\n"
    );
    output.push_str(
        "╠═══════════════════════════════════════════════════════════════════════════════════════════╣\n"
    );
    output.push_str(&format!("║ {:<76}  ║\n", spread_info));
    output.push_str(
        "╚═══════════════════════════════════════════════════════════════════════════════════════════╝\n"
    );

    // Print the detailed orderbook table only if requested
    if show_detailed_orderbook {
        // Print the depth chart header
        output.push_str(
            "\n╔═══════════════════════════════════════════════════════════════════════════════════╗\n"
        );
        output.push_str(&format!(
            "║{}                               DEPTH CHART                                         {}║\n",
            BOLD, RESET
        ));
        output.push_str(
            "╠═══════════════════════════════════════════════════════════════════════════════════╣\n"
        );
        output.push_str(&format!(
            "║{}          BIDS (Buy Orders)            {}║{}          ASKS (Sell Orders)               {}║\n",
            GREEN, RESET, RED, RESET
        ));
        output.push_str(
            "╠═════════╦════════╦═════════╦══════════╬═════════╦════════╦═════════╦══════════════╣\n"
        );
        output.push_str(
            "║ Price   ║ ETH    ║ USD Vol ║ Trader   ║ Price   ║ ETH    ║ USD Vol ║ Trader       ║\n"
        );
        output.push_str(
            "╠═════════╬════════╬═════════╬══════════╬═════════╬════════╬═════════╬══════════════╣\n"
        );

        // Determine how many rows to display
        let max_rows = std::cmp::max(bids.len(), asks.len());

        // Print the depth chart rows
        for i in 0..max_rows {
            let bid_info = if i < bids.len() {
                format!(
                    "║{} {:<7.2} ║ {:<6.2} ║ {:<7.2} ║ {:<8} {}║",
                    GREEN, bid_prices[i], bid_volumes[i], bid_cumulative[i], bid_traders[i], RESET
                )
            } else {
                "║         ║        ║         ║          ║".to_string()
            };

            let ask_info = if i < asks.len() {
                format!(
                    " {:<7.2} ║ {:<6.2} ║ {:<7.2} ║ {:<12} ║",
                    ask_prices[i], ask_volumes[i], ask_cumulative[i], ask_traders[i]
                )
            } else {
                "         ║        ║         ║              ║".to_string()
            };

            output.push_str(&format!("{}{}\n", bid_info, ask_info));
        }

        output.push_str(
            "╚═════════╩════════╩═════════╩══════════╩═════════╩════════╩═════════╩══════════════╝\n"
        );
    }

    // Create a visual representation of the depth chart
    output.push_str("\n╔══════════════════════════════════════════════════════════╗\n");
    output.push_str("║                 VISUAL DEPTH CHART                       ║\n");
    output.push_str("╚══════════════════════════════════════════════════════════╝\n");

    // Define chart dimensions
    let chart_width = 60;
    let chart_height = 20;

    // Create price range
    let min_price = if !bid_prices.is_empty() && !ask_prices.is_empty() {
        f64::min(
            *bid_prices.last().unwrap_or(&0.0),
            *ask_prices.first().unwrap_or(&0.0),
        )
    } else if !bid_prices.is_empty() {
        *bid_prices.last().unwrap_or(&0.0)
    } else if !ask_prices.is_empty() {
        *ask_prices.first().unwrap_or(&0.0)
    } else {
        0.0
    };

    let max_price = if !bid_prices.is_empty() && !ask_prices.is_empty() {
        f64::max(
            *bid_prices.first().unwrap_or(&0.0),
            *ask_prices.last().unwrap_or(&0.0),
        )
    } else if !bid_prices.is_empty() {
        *bid_prices.first().unwrap_or(&0.0)
    } else if !ask_prices.is_empty() {
        *ask_prices.last().unwrap_or(&0.0)
    } else {
        0.0
    };

    // Add some padding to the price range
    let price_range = max_price - min_price;
    let min_price = min_price - (price_range * 0.05);
    let max_price = max_price + (price_range * 0.05);

    // Create a 2D grid for the chart
    let mut chart = vec![vec![' '; chart_width]; chart_height];

    // Draw the axes
    for y in 0..chart_height {
        chart[y][0] = '│';
    }
    for x in 0..chart_width {
        chart[chart_height - 1][x] = '─';
    }
    chart[chart_height - 1][0] = '└';

    // Draw the bid side (cumulative volume) - mark with 'B' for bids
    if !bid_cumulative.is_empty() {
        for i in 0..bid_prices.len() {
            let price_pos = ((bid_prices[i] - min_price) / (max_price - min_price)
                * (chart_width as f64 - 1.0)) as usize;
            let vol_pos = chart_height
                - 1
                - ((bid_cumulative[i] / max_cumulative) * (chart_height as f64 - 1.0)) as usize;
            if price_pos < chart_width && vol_pos < chart_height {
                chart[vol_pos][price_pos] = 'B'; // Mark as bid
            }
        }
    }

    // Draw the ask side (cumulative volume) - mark with 'A' for asks
    if !ask_cumulative.is_empty() {
        for i in 0..ask_prices.len() {
            let price_pos = ((ask_prices[i] - min_price) / (max_price - min_price)
                * (chart_width as f64 - 1.0)) as usize;
            let vol_pos = chart_height
                - 1
                - ((ask_cumulative[i] / max_cumulative) * (chart_height as f64 - 1.0)) as usize;
            if price_pos < chart_width && vol_pos < chart_height {
                chart[vol_pos][price_pos] = 'A'; // Mark as ask
            }
        }
    }

    // Print the chart
    output.push_str("  Volume\n");
    for y in 0..chart_height {
        output.push_str(&format!(
            "{:>8} ",
            if y == 0 {
                format!("{:.1}", max_cumulative)
            } else if y == chart_height - 1 {
                "0.0".to_string()
            } else if y == chart_height / 2 {
                format!("{:.1}", max_cumulative / 2.0)
            } else {
                "".to_string()
            }
        ));

        for x in 0..chart_width {
            match chart[y][x] {
                'B' => output.push_str(&format!("{}█{}", GREEN, RESET)), // Green for bids
                'A' => output.push_str(&format!("{}█{}", RED, RESET)),   // Red for asks
                c => output.push(c),
            }
        }
        output.push('\n');
    }

    // Print the price axis
    output.push_str("         ");
    let mut x = 0;
    while x < chart_width {
        if x == 0 || x == chart_width - 1 || x == chart_width / 2 {
            let price = min_price + (x as f64 / (chart_width - 1) as f64) * (max_price - min_price);
            output.push_str(&format!("{:.0}", price));
            x += 4; // Skip a few positions to avoid overlap
        } else if x % 10 == 0 {
            output.push('│');
            x += 1;
        } else {
            output.push(' ');
            x += 1;
        }
    }
    output.push_str("\n         Price (USDC per ETH)\n");

    // Print summary statistics
    output.push_str("\n╔══════════════════════════════════════════════════════════╗\n");
    output.push_str("║                  ORDERBOOK STATISTICS                    ║\n");
    output.push_str("╠══════════════════════════════╦═══════════════════════╣\n");
    output.push_str(&format!(
        "║ Total number of orders           ║ {:<19}   ║\n",
        swap_notes.len()
    ));
    output.push_str(&format!(
        "║ Number of bid orders             ║ {:<19}   ║\n",
        bids.len()
    ));
    output.push_str(&format!(
        "║ Number of ask orders             ║ {:<19}   ║\n",
        asks.len()
    ));
    output.push_str(&format!(
        "║ Total bid volume (USD)           ║ ${:<18.4}   ║\n",
        bid_cumulative.last().unwrap_or(&0.0)
    ));
    output.push_str(&format!(
        "║ Total ask volume (USD)           ║ ${:<18.4}   ║\n",
        ask_cumulative.last().unwrap_or(&0.0)
    ));
    output.push_str("╚══════════════════════════════════╩═══════════════════════╝\n");

    output
}
