#!/bin/bash

echo "🚀 Miden CLOB Demo Setup"
echo "========================"

# Check if Cargo is available
if ! command -v cargo &> /dev/null; then
    echo "❌ Cargo is not installed. Please install Rust first."
    exit 1
fi

echo "📦 Building the project..."
cargo build --release
if [ $? -ne 0 ]; then
    echo "❌ Build failed"
    exit 1
fi

echo "✅ Build completed successfully!"
echo ""

echo "🗂️  Setting up database..."
# Remove old database if it exists
if [ -f "store.sqlite3" ]; then
    echo "🧹 Removing old database..."
    rm store.sqlite3
fi

echo "🔧 Database will be created automatically when server starts"
echo ""

echo "📖 Demo Instructions:"
echo "====================="
echo ""
echo "⚠️  IMPORTANT: The system now has TWO components that must run separately:"
echo ""
echo "1. Start the CLOB server (thread-safe HTTP API) in one terminal:"
echo "   cargo run --bin server"
echo ""
echo "2. Start the matching engine (blockchain transactions) in another terminal:"
echo "   cargo run --release --bin matching_engine"
echo ""
echo "3. Setup accounts and faucets (one time only):"
echo "   cargo run --release --bin populate -- --setup"
echo ""
echo "4. Populate orders (optional):"
echo "   cargo run --release --bin populate -- --once"
echo ""
echo "5. Test the API:"
echo "   cargo run --example api_example"
echo ""
echo "6. Check the API endpoints:"
echo "   curl http://localhost:3000/health"
echo "   curl http://localhost:3000/stats"
echo ""
echo "🌟 The server will be available at http://localhost:3000"
echo "🤖 The matching engine runs blockchain transactions automatically"
echo "📚 See README_SERVER.md for full documentation"
echo ""
echo "Press any key to continue..."
read -n 1 -s

echo "🎯 Starting demo..."
echo ""
echo "Would you like to:"
echo "1) Start the server"
echo "2) Start the matching engine"
echo "3) Setup accounts (one time only)"
echo "4) Populate orders (server must be running)"
echo "5) Run API example (server must be running)"
echo "6) Exit"
echo ""
read -p "Choose option (1-6): " choice

case $choice in
    1)
        echo "🚀 Starting CLOB server..."
        echo "ℹ️  Don't forget to start the matching engine in another terminal!"
        cargo run --bin server
        ;;
    2)
        echo "🤖 Starting matching engine..."
        echo "ℹ️  Make sure the .env file exists (run setup first if needed)"
        cargo run --bin matching_engine
        ;;
    3)
        echo "🔧 Setting up accounts and faucets..."
        cargo run --bin populate -- --setup
        ;;
    4)
        echo "📊 Populating orders..."
        cargo run --bin populate -- --once
        ;;
    5)
        echo "🧪 Running API example..."
        cargo run --example api_example
        ;;
    6)
        echo "👋 Goodbye!"
        exit 0
        ;;
    *)
        echo "❌ Invalid option"
        exit 1
        ;;
esac