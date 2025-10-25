# Ripgrep Memory-Mapped Search Server

A high-performance search server that uses ripgrep's internals to search memory-mapped files in a codebase. Eliminates subprocess overhead for repeated searches by keeping all files mapped in memory.

## Features

- **In-Memory Search**: Memory maps all files at startup for zero I/O overhead
- **Ripgrep-Powered**: Uses ripgrep's proven `grep-searcher` crate for fast regex matching
- **Parallel Search**: Searches multiple files concurrently using Rayon
- **Respects .gitignore**: Uses the `ignore` crate to skip ignored files
- **HTTP API**: Simple REST API for easy integration
- **Smart Filtering**: Skips binary files and very large files automatically
- **Reload Support**: Can reload the file cache without restarting the server

## Performance

Target: **1-3ms per search operation** vs 5-15ms for subprocess-based tools

Benefits:
- No process spawn overhead
- Files already in memory
- Parallel search across all files
- Optimized regex matching from ripgrep

## Installation

```bash
cd ripgrep-mmap-server
cargo build --release
```

## Usage

### Start the server

```bash
# Index current directory
./target/release/ripgrep-mmap-server

# Index specific directory
./target/release/ripgrep-mmap-server /path/to/codebase

# Set custom port
PORT=8080 ./target/release/ripgrep-mmap-server /path/to/codebase
```

### API Endpoints

#### Health Check
```bash
curl http://localhost:3000/health
```

#### Search (GET)
```bash
# Basic search
curl "http://localhost:3000/search?pattern=function"

# Case-sensitive search
curl "http://localhost:3000/search?pattern=MyClass&case_sensitive=true"

# Limit results
curl "http://localhost:3000/search?pattern=TODO&max_results=50"

# Regex search
curl "http://localhost:3000/search?pattern=fn\s+\w+"
```

#### Search (POST)
```bash
curl -X POST http://localhost:3000/search \
  -H "Content-Type: application/json" \
  -d '{
    "pattern": "struct.*{",
    "case_sensitive": false,
    "max_results": 100
  }'
```

Response format:
```json
{
  "matches": [
    {
      "path": "src/main.rs",
      "line_number": 42,
      "line": "struct MyStruct {",
      "byte_offset": 0
    }
  ],
  "total_matches": 15,
  "files_searched": 347,
  "duration_ms": 2
}
```

#### Reload Cache
```bash
# Reload all files (e.g., after git pull)
curl -X POST http://localhost:3000/reload
```

## Configuration

### Environment Variables

- `PORT`: Server port (default: 3000)
- `RUST_LOG`: Log level (default: info, options: debug, info, warn, error)

```bash
RUST_LOG=debug ./target/release/ripgrep-mmap-server
```

### File Filtering

The server automatically:
- Respects `.gitignore` files
- Skips binary files (png, jpg, pdf, zip, etc.)
- Skips files larger than 50MB
- Skips empty files

## Integration Example

### Python Client

```python
import requests

def search_codebase(pattern, case_sensitive=False, max_results=1000):
    response = requests.post(
        "http://localhost:3000/search",
        json={
            "pattern": pattern,
            "case_sensitive": case_sensitive,
            "max_results": max_results
        }
    )
    return response.json()

# Usage
results = search_codebase(r"class \w+:")
print(f"Found {results['total_matches']} matches in {results['duration_ms']}ms")
for match in results['matches']:
    print(f"{match['path']}:{match['line_number']}: {match['line']}")
```

### Shell Script

```bash
#!/bin/bash
# search.sh - Simple wrapper for the search API

PATTERN="$1"
curl -s "http://localhost:3000/search?pattern=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$PATTERN'))")" | jq
```

## Architecture

```
┌─────────────────────────────────────────┐
│         HTTP Server (Axum)              │
│  GET  /search   - Search via query      │
│  POST /search   - Search via JSON       │
│  POST /reload   - Reload file cache     │
└─────────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────┐
│         MmapCache (In-Memory)           │
│  ┌──────────────────────────────────┐   │
│  │ file1.rs → Mmap (memory-mapped)  │   │
│  │ file2.py → Mmap (memory-mapped)  │   │
│  │ file3.js → Mmap (memory-mapped)  │   │
│  │            ...                    │   │
│  └──────────────────────────────────┘   │
└─────────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────┐
│    Parallel Search (Rayon + Ripgrep)    │
│                                         │
│  ┌─────┐  ┌─────┐  ┌─────┐  ┌─────┐   │
│  │ T1  │  │ T2  │  │ T3  │  │ T4  │   │
│  └─────┘  └─────┘  └─────┘  └─────┘   │
│     │        │        │        │       │
│     ▼        ▼        ▼        ▼       │
│  search_slice() on each mmap           │
└─────────────────────────────────────────┘
```

## Crates Used

- **grep-searcher**: Core search engine from ripgrep
- **grep-regex**: Regex matcher implementation
- **grep-matcher**: Matcher trait interface
- **memmap2**: Memory mapping files
- **ignore**: Directory walking with .gitignore support
- **axum**: HTTP server framework
- **rayon**: Parallel search across files
- **tokio**: Async runtime

## Limitations

- **Memory usage**: All files are kept in memory (respects 50MB per-file limit)
- **File changes**: Doesn't auto-detect file changes (use `/reload` endpoint)
- **Large codebases**: May need tuning for very large codebases (>10GB)

## Performance Tips

1. **Increase max file size** if needed (edit line 103 in main.rs)
2. **Adjust max_results** based on your needs (lower = faster)
3. **Use specific patterns** to reduce matches
4. **Run on SSD** for faster initial mmap

## Benchmarking

### Quick Benchmark

Use the included benchmark tool to compare performance:

```bash
# Run benchmark on current directory
./bench.sh

# Benchmark specific directory and pattern
./bench.sh /path/to/codebase "pattern" 10

# Full usage
./bench.sh <directory> <pattern> <iterations>
```

The benchmark tool tests:
1. **In-Memory Search** - Direct search on memory-mapped files
2. **Ripgrep Subprocess** - Standard ripgrep for comparison
3. **HTTP API** - End-to-end performance (if server is running)

Example output:
```
================================================================================
In-Memory Search (Direct)
================================================================================
Iterations:      10
Total matches:   37
Average time:    0.33ms
Min time:        0.17ms
Max time:        0.54ms
Std deviation:   0.13ms
Median time:     0.33ms
================================================================================

================================================================================
Ripgrep Subprocess (Baseline)
================================================================================
Average time:    11.13ms
...

================================================================================
SPEEDUP ANALYSIS
================================================================================
Memory-mapped vs Ripgrep subprocess: 34.08x faster
Time saved per search: 10.80ms
================================================================================
```

Expected speedup: **10-50x** for repeated searches depending on codebase size

### Manual Comparison

You can also compare manually:

```bash
# Regular ripgrep
time rg "pattern" /path/to/codebase

# Memory-mapped server (after startup)
time curl -s "http://localhost:3000/search?pattern=pattern" > /dev/null
```

## Development

### Build and run
```bash
cargo build
cargo run -- /path/to/codebase
```

### Run with debug logging
```bash
RUST_LOG=debug cargo run -- /path/to/codebase
```

### Run tests (when implemented)
```bash
cargo test
```

## License

MIT (or match your project's license)

## Credits

Built using [ripgrep](https://github.com/BurntSushi/ripgrep)'s excellent search crates by BurntSushi.
