#!/bin/bash
# Benchmark script for ripgrep-mmap-server

set -e

# Default values
DIR="${1:-.}"
PATTERN="${2:-use}"
ITERATIONS="${3:-10}"

# Build release version if not already built
if [ ! -f "target/release/examples/benchmark" ]; then
    echo "Building benchmark..."
    cargo build --example benchmark --release
fi

echo ""
echo "Running benchmark..."
echo "  Directory:  $DIR"
echo "  Pattern:    $PATTERN"
echo "  Iterations: $ITERATIONS"
echo ""

# Run the benchmark
./target/release/examples/benchmark "$DIR" "$PATTERN" "$ITERATIONS"
