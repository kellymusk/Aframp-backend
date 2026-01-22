#!/bin/bash

echo "Testing Aframp Backend Health Check..."
echo ""

# Start the server in background
echo "Starting server..."
cargo run > /tmp/aframp-server.log 2>&1 &
SERVER_PID=$!

# Wait for server to start
sleep 3

# Test health endpoint
echo "Testing health endpoint..."
RESPONSE=$(curl -s http://localhost:8000/health)

if [ $? -eq 0 ]; then
    echo "✅ Health check successful!"
    echo ""
    echo "Response:"
    echo "$RESPONSE" | jq
    echo ""
else
    echo "❌ Health check failed!"
fi

# Stop the server
echo "Stopping server..."
kill $SERVER_PID
wait $SERVER_PID 2>/dev/null

echo "Done!"
