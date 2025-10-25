#!/bin/bash
# CURSERVE Demo Script
# This script demonstrates the full CURSERVE system

set -e

echo "================================================================================"
echo "CURSERVE Demo - High-Performance Coding Agent Serving Engine"
echo "================================================================================"
echo ""

# Check if service is built
if [ ! -f "target/release/mem-search-service" ]; then
    echo "[1] Building service..."
    cargo build --release
    echo "    ✓ Build complete"
    echo ""
fi

# Check if service is already running
if [ -e "/tmp/mem_search_service_requests.sock" ]; then
    echo "⚠️  Service socket already exists. Cleaning up..."
    rm /tmp/mem_search_service_requests.sock
    echo ""
fi

# Start service in background
echo "[2] Starting memory search service..."
./target/release/mem-search-service &
SERVICE_PID=$!

# Wait for service to start
sleep 1

if ! kill -0 $SERVICE_PID 2>/dev/null; then
    echo "❌ Service failed to start"
    exit 1
fi

echo "    ✓ Service running (PID: $SERVICE_PID)"
echo ""

# Run test client
echo "[3] Running test client..."
echo ""
uv run python test_client.py ${1:-.}

# Cleanup
echo ""
echo "[4] Cleaning up..."
kill $SERVICE_PID 2>/dev/null || true
wait $SERVICE_PID 2>/dev/null || true
rm -f /tmp/mem_search_service_requests.sock
rm -f /tmp/qwen_code_response_*.sock

echo "    ✓ Cleanup complete"
echo ""
echo "================================================================================"
echo "Demo complete!"
echo "================================================================================"
