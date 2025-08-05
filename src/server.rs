use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

use crate::{
    database::{Database, SwapNoteRecord},
    note_serialization::deserialize_note,
    orderbook::OrderBookManager,
};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub orderbook_manager: Arc<RwLock<OrderBookManager>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitOrderRequest {
    pub note_data: String, // Base64 encoded serialized note
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitOrderResponse {
    pub success: bool,
    pub order_id: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderBookResponse {
    pub bids: Vec<OrderLevel>,
    pub asks: Vec<OrderLevel>,
    pub spread: Option<f64>,
    pub mid_price: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderLevel {
    pub price: f64,
    pub quantity: f64,
    pub cumulative_quantity: f64,
    pub order_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserOrdersResponse {
    pub orders: Vec<UserOrder>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserOrder {
    pub id: String,
    pub note_id: String,
    pub side: String, // "buy" or "sell"
    pub price: f64,
    pub quantity: f64,
    pub filled_quantity: f64,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DepthChartResponse {
    pub bids: Vec<DepthLevel>,
    pub asks: Vec<DepthLevel>,
    pub market_info: MarketInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DepthLevel {
    pub price: f64,
    pub quantity: f64,
    pub cumulative_quantity: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MarketInfo {
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub spread: Option<f64>,
    pub spread_percentage: Option<f64>,
    pub mid_price: Option<f64>,
    pub total_bid_volume: f64,
    pub total_ask_volume: f64,
}

#[derive(Debug, Deserialize)]
pub struct OrderBookQuery {
    pub base_asset: String,
    pub quote_asset: String,
}

#[derive(Debug, Deserialize)]
pub struct UserOrdersQuery {
    pub user_id: String,
}

#[derive(Debug, Deserialize)]
pub struct DepthChartQuery {
    pub base: Option<String>,
    pub quote: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FrontendDepthChartResponse {
    pub bids: Vec<FrontendOrderBookEntry>,
    pub asks: Vec<FrontendOrderBookEntry>,
    pub spread: f64,
    pub spread_percentage: f64,
    pub last_price: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FrontendOrderBookEntry {
    pub price: f64,
    pub amount: f64,
    pub total: f64,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/orders/submit", post(submit_order))
        .route("/orderbook", get(get_orderbook))
        .route("/orders/user", get(get_user_orders))
        .route("/depth/:base/:quote", get(get_depth_chart))
        .route("/api/depth-chart", get(get_frontend_depth_chart))
        .route("/stats", get(get_stats))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now()
    }))
}

async fn submit_order(
    State(state): State<AppState>,
    Json(payload): Json<SubmitOrderRequest>,
) -> Json<SubmitOrderResponse> {
    info!("Received order submission request");

    // Deserialize the note from base64
    let note = match deserialize_note(&payload.note_data) {
        Ok(note) => note,
        Err(e) => {
            error!("Failed to deserialize note: {}", e);
            return Json(SubmitOrderResponse {
                success: false,
                order_id: "".to_string(),
                message: format!("Invalid note data: {}", e),
            });
        }
    };

    // Add to order book manager (thread-safe, no blockchain interaction)
    let mut manager = state.orderbook_manager.write().await;
    match manager.add_swap_note(note, &state.db).await {
        Ok(order_id) => {
            info!("Successfully added order: {}", order_id);
            Json(SubmitOrderResponse {
                success: true,
                order_id,
                message:
                    "Order submitted successfully. Matching engine will process automatically."
                        .to_string(),
            })
        }
        Err(e) => {
            error!("Failed to add order: {}", e);
            Json(SubmitOrderResponse {
                success: false,
                order_id: "".to_string(),
                message: format!("Failed to submit order: {}", e),
            })
        }
    }
}

async fn get_orderbook(
    State(state): State<AppState>,
    Query(params): Query<OrderBookQuery>,
) -> Result<Json<OrderBookResponse>, (StatusCode, String)> {
    match state
        .db
        .get_orderbook(&params.base_asset, &params.quote_asset)
        .await
    {
        Ok((bids, asks)) => {
            let bid_levels = aggregate_orders(&bids);
            let ask_levels = aggregate_orders(&asks);

            let spread = if !bid_levels.is_empty() && !ask_levels.is_empty() {
                Some(ask_levels[0].price - bid_levels[0].price)
            } else {
                None
            };

            let mid_price = if !bid_levels.is_empty() && !ask_levels.is_empty() {
                Some((bid_levels[0].price + ask_levels[0].price) / 2.0)
            } else {
                None
            };

            Ok(Json(OrderBookResponse {
                bids: bid_levels,
                asks: ask_levels,
                spread,
                mid_price,
            }))
        }
        Err(e) => {
            error!("Failed to get orderbook: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get orderbook: {}", e),
            ))
        }
    }
}

async fn get_user_orders(
    State(state): State<AppState>,
    Query(params): Query<UserOrdersQuery>,
) -> Result<Json<UserOrdersResponse>, (StatusCode, String)> {
    match state.db.get_user_orders(&params.user_id).await {
        Ok(orders) => {
            let user_orders = orders
                .into_iter()
                .map(|order| UserOrder {
                    id: order.id,
                    note_id: order.note_id,
                    side: if order.is_bid {
                        "buy".to_string()
                    } else {
                        "sell".to_string()
                    },
                    price: order.price,
                    quantity: order.offered_amount as f64,
                    filled_quantity: 0.0, // TODO: Calculate filled quantity
                    status: format!("{:?}", order.status),
                    created_at: order.created_at.to_rfc3339(),
                    updated_at: order.updated_at.to_rfc3339(),
                })
                .collect();

            Ok(Json(UserOrdersResponse {
                orders: user_orders,
            }))
        }
        Err(e) => {
            error!("Failed to get user orders: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get user orders: {}", e),
            ))
        }
    }
}

async fn get_depth_chart(
    State(state): State<AppState>,
    Path((base_asset, quote_asset)): Path<(String, String)>,
) -> Result<Json<DepthChartResponse>, (StatusCode, String)> {
    match state.db.get_orderbook(&base_asset, &quote_asset).await {
        Ok((bids, asks)) => {
            let mut bid_levels = Vec::new();
            let mut ask_levels = Vec::new();

            // Convert bids to depth levels
            let mut cumulative_bid_volume = 0.0;
            for bid in &bids {
                let quantity = bid.offered_amount as f64;
                cumulative_bid_volume += quantity;
                bid_levels.push(DepthLevel {
                    price: bid.price,
                    quantity,
                    cumulative_quantity: cumulative_bid_volume,
                });
            }

            // Convert asks to depth levels
            let mut cumulative_ask_volume = 0.0;
            for ask in &asks {
                let quantity = ask.offered_amount as f64;
                cumulative_ask_volume += quantity;
                ask_levels.push(DepthLevel {
                    price: ask.price,
                    quantity,
                    cumulative_quantity: cumulative_ask_volume,
                });
            }

            let best_bid = bids.first().map(|b| b.price);
            let best_ask = asks.first().map(|a| a.price);

            let (spread, spread_percentage, mid_price) = match (best_bid, best_ask) {
                (Some(bid), Some(ask)) => {
                    let spread = ask - bid;
                    let spread_pct = (spread / bid) * 100.0;
                    let mid = (bid + ask) / 2.0;
                    (Some(spread), Some(spread_pct), Some(mid))
                }
                _ => (None, None, None),
            };

            let market_info = MarketInfo {
                best_bid,
                best_ask,
                spread,
                spread_percentage,
                mid_price,
                total_bid_volume: cumulative_bid_volume,
                total_ask_volume: cumulative_ask_volume,
            };

            Ok(Json(DepthChartResponse {
                bids: bid_levels,
                asks: ask_levels,
                market_info,
            }))
        }
        Err(e) => {
            error!("Failed to get depth chart: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get depth chart: {}", e),
            ))
        }
    }
}

async fn get_frontend_depth_chart(
    State(state): State<AppState>,
    Query(params): Query<DepthChartQuery>,
) -> Result<Json<FrontendDepthChartResponse>, (StatusCode, String)> {
    let _base_asset = params.base.unwrap_or_else(|| "ETH".to_string());
    let _quote_asset = params.quote.unwrap_or_else(|| "USDC".to_string());

    match state.db.get_open_swap_notes().await {
        Ok(open_orders) => {
            info!("Found {} open orders in database", open_orders.len());

            let mut bids = Vec::new();
            let mut asks = Vec::new();

            for order in &open_orders {
                info!(
                    "Processing order: {} (is_bid: {}, price: {}, raw_amount: {})",
                    order.note_id, order.is_bid, order.price, order.offered_amount
                );

                if order.is_bid {
                    let price = if order.price > 0.0 {
                        1.0 / order.price
                    } else {
                        0.0
                    };
                    let amount = order.offered_amount as f64 * price / 1e9;
                    info!("BID - original price: {}, inverted price: {}, raw_amount: {}, converted_amount: {}",
                          order.price, price, order.offered_amount, amount);
                    bids.push((price, amount));
                } else {
                    let price = order.price;
                    let amount = order.offered_amount as f64 / 100.0;
                    info!(
                        "ASK - price: {}, raw_amount: {}, converted_amount: {}",
                        price, order.offered_amount, amount
                    );
                    asks.push((price, amount));
                }
            }

            info!("Processed orders: {} bids, {} asks", bids.len(), asks.len());

            // Sort bids by price (descending - highest first)
            bids.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

            // Sort asks by price (ascending - lowest first)
            asks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

            // Calculate cumulative totals for bids
            let mut processed_bids = Vec::new();
            let mut cumulative_bid_amount = 0.0;

            for (price, amount) in bids {
                cumulative_bid_amount += amount;
                processed_bids.push(FrontendOrderBookEntry {
                    price,
                    amount,
                    total: cumulative_bid_amount,
                });
            }

            // Calculate cumulative totals for asks
            let mut processed_asks = Vec::new();
            let mut cumulative_ask_amount = 0.0;

            for (price, amount) in asks {
                cumulative_ask_amount += amount;
                processed_asks.push(FrontendOrderBookEntry {
                    price,
                    amount,
                    total: cumulative_ask_amount,
                });
            }

            // Calculate market metrics
            let best_bid = processed_bids.first().map(|b| b.price);
            let best_ask = processed_asks.first().map(|a| a.price);

            let (spread, spread_percentage, last_price) = match (best_bid, best_ask) {
                (Some(bid), Some(ask)) => {
                    let spread = ask - bid;
                    let spread_pct = if bid > 0.0 {
                        (spread / bid) * 100.0
                    } else {
                        0.0
                    };
                    let mid = (bid + ask) / 2.0;
                    (spread, spread_pct, mid)
                }
                (Some(bid), None) => (0.0, 0.0, bid),
                (None, Some(ask)) => (0.0, 0.0, ask),
                (None, None) => (0.0, 0.0, 0.0),
            };

            info!(
                "Returning depth chart: {} bids, {} asks, spread: {:.2}, last_price: {:.2}",
                processed_bids.len(),
                processed_asks.len(),
                spread,
                last_price
            );

            Ok(Json(FrontendDepthChartResponse {
                bids: processed_bids,
                asks: processed_asks,
                spread,
                spread_percentage,
                last_price,
            }))
        }
        Err(e) => {
            error!("Failed to get open swap notes: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get depth chart: {}", e),
            ))
        }
    }
}

async fn get_stats(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    match state.db.get_open_swap_notes().await {
        Ok(notes) => {
            let total_orders = notes.len();
            let bid_count = notes.iter().filter(|n| n.is_bid).count();
            let ask_count = notes.iter().filter(|n| !n.is_bid).count();

            Ok(Json(serde_json::json!({
                "total_orders": total_orders,
                "bid_count": bid_count,
                "ask_count": ask_count,
                "timestamp": chrono::Utc::now()
            })))
        }
        Err(e) => {
            error!("Failed to get stats: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get stats: {}", e),
            ))
        }
    }
}

fn aggregate_orders(orders: &[SwapNoteRecord]) -> Vec<OrderLevel> {
    use std::collections::HashMap;

    let mut price_levels: HashMap<u64, (f64, usize)> = HashMap::new();

    // Group orders by price (rounded to avoid floating point issues)
    for order in orders {
        let price_key = (order.price * 100.0) as u64; // Round to 2 decimal places
        let quantity = order.offered_amount as f64;

        let entry = price_levels.entry(price_key).or_insert((0.0, 0));
        entry.0 += quantity;
        entry.1 += 1;
    }

    // Convert to sorted vector
    let mut levels: Vec<_> = price_levels
        .into_iter()
        .map(|(price_key, (quantity, count))| {
            let price = (price_key as f64) / 100.0;
            (price, quantity, count)
        })
        .collect();

    // Sort by price (descending for bids, ascending for asks)
    levels.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

    // Calculate cumulative quantities
    let mut cumulative = 0.0;
    levels
        .into_iter()
        .map(|(price, quantity, count)| {
            cumulative += quantity;
            OrderLevel {
                price,
                quantity,
                cumulative_quantity: cumulative,
                order_count: count,
            }
        })
        .collect()
}
