# MONO: High-Performance Coding Agent Serving Engine

MONO eliminates the ripgrep bottleneck in LLM coding agents by keeping codebases memory-mapped and executing searches in-memory. Instead of spawning a subprocess for every `rg` call (~10-15ms overhead), MONO searches directly in RAM (~0.5-3ms).

## The Problem

Traditional coding agents (like Claude Code, Cursor, etc.) have a major performance bottleneck:

```
┌─────────┐                    ┌─────────┐
│  Client │ ◄─────────────────►│  Server │
│(Laptop) │    Network Calls   │  (LLM)  │
└─────────┘                    └─────────┘
     │
     ▼
  Codebase
  (Local Files)
```

**Every ripgrep call requires:**
1. LLM generates tool call → Network → Client
2. Client spawns `rg` subprocess → ~10-15ms overhead
3. Results → Network → LLM
4. Repeat 10-20 times per agent turn (ripgrep is the most common tool)

**Total overhead:** 100-300ms per agent turn, dominated by subprocess spawn time

## The Solution

MONO co-locates codebases with the LLM server and uses in-memory operations:

```
┌─────────────────────────────────────┐
│          Server                     │
│  ┌─────────┐    ┌──────────────┐    │
│  │  vLLM   │───►│ Mem Search   │    │
│  │ (Qwen)  │    │   Service    │    │
│  └─────────┘    └──────────────┘    │
│                        │            │
│                        ▼            │
│                  ┌──────────┐       │
│                  │ Codebases│       │
│                  │(Memory)  │       │
│                  └──────────┘       │
└─────────────────────────────────────┘
         ▲
         │ SSH
         │
    ┌─────────┐
    │ Client  │
    └─────────┘
```

**Benefits:**
- No subprocess spawn overhead (~10-15ms saved per search)
- In-memory ripgrep: **0.5-3ms** vs subprocess: **10-15ms**
- **10-50x speedup** for search operations
- Multi-tenant: one service handles multiple qwen-code instances

## Architecture

### Components

1. **mem-search-service** (Rust daemon)
   - Memory-maps all active codebases into RAM
   - Provides IPC interface via Unix domain sockets
   - Handles concurrent requests from multiple clients
   - Executes ripgrep search directly in memory using ripgrep internals

2. **mono_client.py** (Python library)
   - Client library for communicating with mem-search-service
   - Can be integrated into qwen-code or other Python agents
   - Simple API: `alloc_pid()`, `ripgrep()`, `close()`

3. **IPC Layer** (Unix domain sockets)
   - Request socket: `/tmp/mem_search_service_requests.sock` (shared)
   - Response sockets: `/tmp/qwen_code_response_{pid}.sock` (per-client)
   - JSON-based protocol

### Request Flow

```
1. Client → alloc_pid(codebase_path)
   ├─ Service memory-maps entire codebase
   ├─ Creates dedicated response socket for client
   └─ Returns success

2. Client → request_ripgrep(pattern)
   ├─ Service searches in-memory files using ripgrep
   ├─ Results formatted like ripgrep output
   └─ Returns via client's response socket

3. Repeat step 2 for each search (no overhead!)
```

## Quick Start

### 1. Build the Service

```bash
cargo build --release
```

### 2. Start the Memory Search Service

```bash
./target/release/mem-search-service
```

Output:
```
================================================================================
MONO Memory Search Service
================================================================================

Request listener started on /tmp/mem_search_service_requests.sock
Worker thread started
Service running. Press Ctrl+C to stop.
```

### 3. Use the Python Client

```python
from mono_client import MemSearchClient

# Create client
client = MemSearchClient()

# Allocate codebase (memory-maps all files)
client.alloc_pid("/path/to/codebase")

# Search (happens in-memory, ~1-3ms)
results = client.ripgrep("use std")
print(results)

# Close
client.close()
```

Or use the convenience function:
```python
from mono_client import ripgrep

results = ripgrep("pattern", "/path/to/codebase")
print(results)
```

### 4. Test It

```bash
# In terminal 1: Start service
./target/release/mem-search-service

# In terminal 2: Run test client
python3 test_client.py /path/to/codebase
```

Expected output:
```
================================================================================
MONO Memory Search Service - Test Client
================================================================================

Codebase: /path/to/codebase

[1] Allocating codebase (memory-mapping files)...
Loading files into memory from: /path/to/codebase
Loaded 347 files (12.45 MB total) into memory
    ✓ Allocated in 45.23ms
    Response: Allocated 347 files

[2] Searching for 'use'...
    ✓ Found 145 matches in 0.52ms

[3] Speed test - 10 repeated searches...
    Average: 0.43ms
    Min:     0.38ms
    Max:     0.52ms

================================================================================
SUCCESS
================================================================================
✓ Memory-mapped search working!
✓ Average search time: 0.43ms
✓ Typical subprocess overhead saved: ~10-15ms per search
================================================================================
```

## IPC Protocol

### Request Types

#### 1. alloc_pid

Allocate and memory-map a codebase for a client.

**Request:**
```json
{
  "type": "alloc_pid",
  "pid": 12345,
  "repo_dir_path": "/path/to/codebase"
}
```

**Response:**
```json
{
  "response_status": 1,
  "text": "Allocated 347 files"
}
```

#### 2. request_ripgrep

Search the allocated codebase.

**Request:**
```json
{
  "type": "request_ripgrep",
  "pid": 12345,
  "pattern": "fn \\w+",
  "case_sensitive": false
}
```

**Response:**
```json
{
  "response_status": 1,
  "text": "src/main.rs:42:fn main() {\nsrc/lib.rs:10:fn search() {"
}
```

### Error Handling

**Response (error):**
```json
{
  "response_status": 0,
  "error": "PID 12345 has no allocated codebase. Call alloc_pid first."
}
```

## Performance

### Benchmark

Run the standalone benchmark:

```bash
./target/release/benchmark /path/to/codebase "pattern" 10
```

Example results:
```
================================================================================
Memory-Mapped Search
================================================================================
  Iteration  1: 0.52ms (145 matches)
  Iteration  2: 0.38ms (145 matches)
  ...
  Iteration 10: 0.41ms (145 matches)

Results:
  Average: 0.43ms
  Min:     0.38ms
  Max:     0.52ms

================================================================================
Ripgrep Subprocess
================================================================================
  Iteration  1: 12.45ms (145 matches)
  ...

Results:
  Average: 12.15ms

================================================================================
SUMMARY
================================================================================
Speedup: 28.26x faster
Time saved per search: 11.72ms
```

### Typical Performance

| Operation | Subprocess | In-Memory | Speedup |
|-----------|-----------|-----------|---------|
| Small codebase (<100 files) | ~10ms | ~0.5ms | 20x |
| Medium codebase (~500 files) | ~15ms | ~1ms | 15x |
| Large codebase (1000+ files) | ~20ms | ~3ms | 7x |

**Impact on agent turns:**
- Traditional: 10 tool calls × 15ms = 150ms overhead
- MONO: 10 tool calls × 1ms = 10ms overhead
- **Savings: 140ms per turn = 93% reduction**

## Integration with Qwen-Code

To integrate with qwen-code, replace only the ripgrep tool call:

```python
# In qwen-code's tool handlers
from mono_client import MemSearchClient

class ModifiedRipgrepHandler:
    def __init__(self, codebase_path):
        self.client = MemSearchClient()
        self.client.alloc_pid(codebase_path)

    def ripgrep(self, pattern):
        # Replace subprocess ripgrep with in-memory search
        return self.client.ripgrep(pattern)

# All other tools (ls, cat, etc.) use default qwen-code implementation
```

## Project Structure

```
mono/
├── src/
│   ├── lib.rs              # Shared MmapCache implementation
│   ├── service.rs          # Memory search service daemon
│   └── benchmark.rs        # Standalone benchmark tool
├── mono_client.py          # Python client library
├── test_client.py          # Test/demo script
├── Cargo.toml              # Rust dependencies
└── README.md               # This file
```

## Dependencies

### Rust
- `grep-searcher` - Ripgrep's core search engine
- `grep-regex` - Regex implementation
- `memmap2` - Memory mapping
- `ignore` - .gitignore support
- `rayon` - Parallel search
- `serde` + `serde_json` - JSON serialization
- `crossbeam-channel` - Thread communication

### Python
- Standard library only (no external dependencies!)

## Future Work

- [ ] Add file watch for auto-reload on changes
- [ ] Add codebase paging/eviction for when RAM is limited
- [ ] Optimize with suffix trees for super-hot files
- [ ] Support for distributed codebases across multiple servers
- [ ] Copy-on-write for shared codebases (e.g., PyTorch team)

## Troubleshooting

**Service won't start:**
- Check if socket already exists: `ls /tmp/mem_search_service_requests.sock`
- Remove it: `rm /tmp/mem_search_service_requests.sock`
- Restart service

**Client can't connect:**
- Make sure service is running in another terminal
- Check service output for errors
- Make sure you're using Python 3: `python3` not `python`

## FAQ

**Q: How much RAM does this use?**
A: Approximately the size of your text files. Binary files are skipped. A typical 10MB codebase uses ~10MB RAM.

**Q: What happens when files change?**
A: In-place edits are visible automatically. However, most text editors atomically replace files (creating new inodes), so you'll need to restart the client and re-allocate. File watching for auto-reload is planned.

**Q: Can multiple clients share the same codebase?**
A: Each client gets its own memory-mapped copy. Copy-on-write sharing is planned.

**Q: Does this work on macOS/Linux/Windows?**
A: Unix domain sockets are macOS/Linux only. Windows support via named pipes is planned.

## License

MIT

## Credits

Built using [ripgrep](https://github.com/BurntSushi/ripgrep)'s excellent search crates by BurntSushi.
