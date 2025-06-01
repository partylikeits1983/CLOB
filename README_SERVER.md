# Miden CLOB Server

A Central Limit Order Book (CLOB) server for Miden that handles SWAP note submissions, order matching, and provides REST API endpoints for order book management.

## Features

- **Order Submission**: Accept serialized SWAP notes and store them in SQLite database
- **Order Matching**: Automatic matching of crossing orders using Miden's SWAP note logic
- **REST API**: Comprehensive API for order book access, user orders, and market data
- **Real-time Matching**: Continuous order matching with transaction submission to Miden network
- **Market Making**: Automated script to populate the order book with realistic orders based on live ETH prices

## Architecture

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Frontend/UI   │◄──►│  CLOB Server    │◄──►│  Miden Network  │
│                 │    │  (REST API)     │    │   (Blockchain)  │
└─────────────────┘    └─────────────────┘    └─────────────────┘
                              │
                              ▼
                       ┌─────────────────┐
                       │ SQLite Database │
                       │ (store.sqlite3) │
                       └─────────────────┘
```

## Quick Start

### 1. Start the CLOB Server

```bash
# Build and run the server
cargo run --bin server

# Server will start on http://localhost:3000
```

### 2. Populate the Order Book

In a separate terminal, run the market maker script:

```bash
# Run once to populate initial orders
cargo run --bin populate -- --once

# Or run continuously for live market making
cargo run --bin populate
```

### 3. Test the API

```bash
# Check server health
curl http://localhost:3000/health

# Get current orderbook (replace with actual asset IDs)
curl "http://localhost:3000/orderbook?base_asset=eth_id&quote_asset=usdc_id"

# Get market depth chart
curl http://localhost:3000/depth/eth_id/usdc_id

# Trigger manual matching
curl -X POST http://localhost:3000/match

# Get server statistics
curl http://localhost:3000/stats
```

## API Endpoints

### Health Check
```
GET /health
```
Returns server status and timestamp.

### Submit Order
```
POST /orders/submit
Content-Type: application/json

{
  "note_data": "base64_encoded_serialized_swap_note"
}
```
Submit a new SWAP note to the order book.

### Get Order Book
```
GET /orderbook?base_asset={base_id}&quote_asset={quote_id}
```
Returns aggregated order book with bids and asks.

### Get User Orders
```
GET /orders/user?user_id={user_account_id}
```
Returns all orders for a specific user.

### Get Market Depth
```
GET /depth/{base_asset}/{quote_asset}
```
Returns detailed market depth chart data.

### Trigger Matching
```
POST /match
```
Manually trigger order matching cycle.

### Get Statistics
```
GET /stats
```
Returns server statistics including order counts.

## Database Schema

### swap_notes table
- `id`: Unique identifier
- `note_id`: Miden note ID
- `creator_id`: Account ID of order creator
- `offered_asset_id`: Asset being sold
- `offered_amount`: Amount being sold
- `requested_asset_id`: Asset being bought
- `requested_amount`: Amount being bought
- `price`: Calculated price
- `is_bid`: Boolean indicating buy (true) or sell (false) order
- `note_data`: Serialized note data
- `status`: Order status (open, filled, cancelled, etc.)
- `created_at`: Timestamp
- `updated_at`: Timestamp

### p2id_notes table
- `id`: Unique identifier
- `note_id`: Miden note ID
- `sender_id`: Sender account ID
- `target_id`: Target account ID
- `asset_id`: Asset being transferred
- `amount`: Amount being transferred
- `swap_note_id`: Related swap note (if applicable)
- `note_data`: Serialized note data
- `created_at`: Timestamp

## Market Maker Configuration

The populate script can be configured with various parameters:

```rust
MarketMakerConfig {
    spread_percentage: 1.0,    // 1% spread around mid price
    num_levels: 3,             // 3 price levels per side
    base_quantity: 0.5,        // 0.5 ETH base order size
    quantity_variance: 0.4,    // ±40% quantity variance
    price_variance: 0.05,      // ±5% price variance
    update_interval_secs: 30,  // Update orders every 30 seconds
}
```

## Order Matching Logic

The server uses the same matching algorithm as your tests:

1. **Order Discovery**: Continuously monitor for new SWAP notes
2. **Cross Detection**: Find orders where bid price ≥ ask price
3. **Match Execution**: Use `try_match_swapp_notes()` from your common module
4. **Transaction Creation**: Build and submit matching transactions to Miden
5. **State Update**: Update database with match results and new P2ID notes

## Integration with Existing Code

The server directly integrates with your existing matching logic:

- Uses `try_match_swapp_notes()` for order matching
- Leverages `create_p2id_note()` for settlement notes
- Integrates with your account setup and faucet management
- Compatible with your existing SWAP note structure

## Development

### Running Tests
```bash
# Run existing tests (includes matching algorithm tests)
cargo test

# Run specific test
cargo test realistic_usdc_eth_orderbook_match
```

### Development Mode
```bash
# Run with debug logging
RUST_LOG=debug cargo run --bin server
```

### Database Management
```bash
# Database is automatically created as store.sqlite3
# To reset the database, simply delete the file:
rm store.sqlite3
```

## Production Considerations

1. **Security**: Implement proper authentication and authorization
2. **Rate Limiting**: Add rate limiting to prevent abuse
3. **Monitoring**: Add comprehensive logging and metrics
4. **Scalability**: Consider database optimizations for high throughput
5. **Error Handling**: Implement robust error handling and recovery
6. **Backup**: Regular database backups
7. **Network**: Proper Miden network configuration for production

## Example Usage Flow

1. **Setup**: Start server and populate initial orders
2. **Order Submission**: Users submit SWAP notes via API
3. **Automatic Matching**: Server continuously matches crossing orders
4. **Settlement**: Matched orders generate P2ID notes for asset transfer
5. **Monitoring**: Use API endpoints to monitor order book and user positions

## Future Enhancements

- [ ] WebSocket support for real-time updates
- [ ] Advanced order types (stop-loss, iceberg orders)
- [ ] Order cancellation support
- [ ] Market data feeds (OHLCV, trades)
- [ ] Fee calculation and collection
- [ ] Admin interface
- [ ] Performance optimization
- [ ] Multi-pair support with automatic price discovery

## Troubleshooting

### Common Issues

1. **Database Connection Errors**: Ensure SQLite permissions are correct
2. **Miden Network Issues**: Check Miden node connectivity
3. **Serialization Errors**: Verify SWAP note format compatibility
4. **Matching Failures**: Check order format and asset compatibility

### Logs
Check server logs for detailed error information:
```bash
RUST_LOG=debug cargo run --bin server