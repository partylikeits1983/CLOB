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
echo "1. Start the CLOB server in one terminal:"
echo "   cargo run --bin server"
echo ""
echo "2. In another terminal, populate orders:"
echo "   cargo run --bin populate -- --once"
echo ""
echo "3. Test the API:"
echo "   cargo run --example api_example"
echo ""
echo "4. Check the API endpoints:"
echo "   curl http://localhost:3000/health"
echo "   curl http://localhost:3000/stats"
echo ""
echo "🌟 The server will be available at http://localhost:3000"
echo "📚 See README_SERVER.md for full documentation"
echo ""
echo "Press any key to continue..."
read -n 1 -s

echo "🎯 Starting demo..."
echo ""
echo "Would you like to:"
echo "1) Start the server"
echo "2) Populate orders (server must be running)"
echo "3) Run API example (server must be running)"
echo "4) Exit"
echo ""
read -p "Choose option (1-4): " choice

case $choice in
    1)
        echo "🚀 Starting CLOB server..."
        cargo run --bin server
        ;;
    2)
        echo "📊 Populating orders..."
        cargo run --bin populate -- --once
        ;;
    3)
        echo "🧪 Running API example..."
        cargo run --example api_example
        ;;
    4)
        echo "👋 Goodbye!"
        exit 0
        ;;
    *)
        echo "❌ Invalid option"
        exit 1
        ;;
esac