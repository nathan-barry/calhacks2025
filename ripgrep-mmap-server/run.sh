#!/bin/bash
# Quick start script for the ripgrep-mmap-server

set -e

# Default to current directory if no argument provided
DIR="${1:-.}"

# Build release version if not already built
if [ ! -f "target/release/ripgrep-mmap-server" ]; then
    echo "Building release version..."
    cargo build --release
fi

echo "Starting ripgrep-mmap-server..."
echo "Indexing directory: $DIR"
echo ""

# Run the server
./target/release/ripgrep-mmap-server "$DIR"
