# Ripgrep Memory-Mapped Search

A simple benchmark comparing in-memory search (using ripgrep's internals with memory-mapped files) vs subprocess-based ripgrep.

## What It Does

1. **Loads files into memory** - Memory-maps all files in a directory (respects .gitignore)
2. **In-memory search** - Uses ripgrep's `search_slice()` to search directly on memory-mapped data
3. **Benchmark** - Compares performance against spawning ripgrep as a subprocess

## Core Functionality

- `MmapCache::new()` - Memory-maps all files in a directory
- `MmapCache::search()` - Searches using ripgrep's grep-searcher crate on mmapped data
- `ripgrep_subprocess()` - Runs `rg` as a subprocess for comparison

## Build

```bash
cargo build --release
```

## Run

```bash
# Basic usage (searches for "use")
./target/release/ripgrep-mmap /path/to/codebase

# Custom pattern
./target/release/ripgrep-mmap /path/to/codebase "fn "

# Custom pattern and iterations
./target/release/ripgrep-mmap /path/to/codebase "use std" 20
```

## How It Works

### Memory Mapping
```rust
// Load all files into memory using mmap
let walker = ignore::WalkBuilder::new(root)
    .git_ignore(true)
    .build();

for file in walker {
    let mmap = unsafe { Mmap::map(&file)? };
    files.insert(path, mmap);
}
```

### In-Memory Search
```rust
// Search using ripgrep's internals on the mmap
searcher.search_slice(
    &matcher,
    &mmap[..],  // Search directly on memory-mapped data
    UTF8(|line_num, line| {
        matches.push((path, line_num, line));
        Ok(true)
    })
)
```

### Subprocess (for comparison)
```rust
// Spawn ripgrep as subprocess
Command::new("rg")
    .arg(pattern)
    .arg(root)
    .output()
```

## Why It's Faster

1. **No subprocess spawn overhead** (~5-10ms per call)
2. **Files already in memory** (no repeated file I/O)
3. **Parallel search** across all files using Rayon
4. **Same regex engine** as ripgrep under the hood

## Dependencies

- `grep-searcher` - Ripgrep's core search engine
- `grep-regex` - Ripgrep's regex implementation
- `memmap2` - Memory mapping
- `ignore` - .gitignore support
- `rayon` - Parallel search
- `anyhow` - Error handling

## Use Case

Perfect for LLM coding agents that need to search a codebase repeatedly:
- Traditional approach: Spawn `rg` subprocess for each search (~10-15ms)
- This approach: Search in-memory (~0.5-3ms)
- **10-30x speedup** for repeated searches
