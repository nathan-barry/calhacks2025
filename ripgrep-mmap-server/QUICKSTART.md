# Quick Start Guide

## Build

```bash
cargo build --release
```

## Run Server

```bash
# Index current directory on port 3000
./run.sh

# Or manually specify directory
./target/release/ripgrep-mmap-server /path/to/codebase
```

## Test Search

```bash
# Simple search
curl "http://localhost:3000/search?pattern=function"

# Regex search
curl "http://localhost:3000/search?pattern=fn\s+\w+"

# Case-sensitive
curl "http://localhost:3000/search?pattern=TODO&case_sensitive=true"
```

## Run Benchmark

```bash
# Quick benchmark (uses current directory)
./bench.sh

# Benchmark specific directory
./bench.sh /path/to/codebase "pattern" 10
```

Expected results:
- In-memory search: **~0.3-3ms**
- Ripgrep subprocess: **~10-15ms**
- Speedup: **10-50x faster**

## Python Client Example

```bash
python3 example_client.py
```

## Key Files

- `src/main.rs` - Main server implementation
- `examples/benchmark.rs` - Comprehensive benchmark tool
- `example_client.py` - Python client with examples
- `run.sh` - Quick start server script
- `bench.sh` - Quick benchmark script

## API Endpoints

- `GET /health` - Health check
- `GET /search?pattern=<regex>` - Search (query params)
- `POST /search` - Search (JSON body)
- `POST /reload` - Reload file cache

## Architecture

1. **Startup**: Memory-map all files (respects .gitignore)
2. **Search**: Use ripgrep's `search_slice()` on each mmap in parallel
3. **Return**: JSON results with matches, line numbers, and timing

## Performance Notes

- Files are kept in memory (50MB max per file)
- Parallel search across all files using Rayon
- Zero I/O overhead after initial load
- No subprocess spawn overhead

## Tips

- Use `POST /reload` after `git pull` to refresh files
- Increase `max_results` parameter for more matches
- Use specific regex patterns to reduce match count
- Run on SSD for faster initial memory mapping
