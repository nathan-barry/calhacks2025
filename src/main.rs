use anyhow::{Context, Result};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use memmap2::Mmap;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Memory-mapped file cache
struct MmapCache {
    files: HashMap<PathBuf, Mmap>,
    root: PathBuf,
}

impl MmapCache {
    /// Create a new cache by memory-mapping all files in the given directory
    fn new(root: &Path) -> Result<Self> {
        println!("Loading files into memory from: {}", root.display());
        let mut files = HashMap::new();
        let mut file_count = 0;
        let mut total_bytes = 0u64;

        // Use ignore crate to walk directory, respecting .gitignore
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let entry = entry.context("Failed to read directory entry")?;

            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();

            // Skip binary files
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy();
                if matches!(
                    ext_str.as_ref(),
                    "png" | "jpg" | "jpeg" | "gif" | "pdf" | "zip" | "tar" | "gz" |
                    "so" | "dylib" | "dll" | "exe" | "bin" | "o" | "a"
                ) {
                    continue;
                }
            }

            match File::open(path) {
                Ok(file) => {
                    let metadata = file.metadata()?;
                    let file_size = metadata.len();

                    // Skip very large files (>50MB) and empty files
                    if file_size > 50 * 1024 * 1024 || file_size == 0 {
                        continue;
                    }

                    match unsafe { Mmap::map(&file) } {
                        Ok(mmap) => {
                            file_count += 1;
                            total_bytes += file_size;
                            files.insert(path.to_owned(), mmap);
                        }
                        Err(_) => continue,
                    }
                }
                Err(_) => continue,
            }
        }

        println!(
            "Loaded {} files ({:.2} MB total) into memory",
            file_count,
            total_bytes as f64 / 1024.0 / 1024.0
        );

        Ok(Self {
            files,
            root: root.to_owned(),
        })
    }

    /// Search all memory-mapped files for the given pattern
    fn search(&self, pattern: &str, case_sensitive: bool) -> Result<Vec<(String, u64, String)>> {
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!case_sensitive)
            .build(pattern)
            .context("Invalid regex pattern")?;

        // Search all files in parallel
        let all_matches: Vec<Vec<(String, u64, String)>> = self
            .files
            .par_iter()
            .map(|(path, mmap)| {
                let mut matches = Vec::new();
                let mut searcher = Searcher::new();

                let rel_path = path
                    .strip_prefix(&self.root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                let _ = searcher.search_slice(
                    &matcher,
                    &mmap[..],
                    UTF8(|line_num, line| {
                        matches.push((rel_path.clone(), line_num, line.trim_end().to_string()));
                        Ok(true)
                    }),
                );

                matches
            })
            .collect();

        Ok(all_matches.into_iter().flatten().collect())
    }
}

/// Run ripgrep as a subprocess for comparison
fn ripgrep_subprocess(root: &Path, pattern: &str) -> Result<Vec<(String, u64, String)>> {
    let output = Command::new("rg")
        .arg(pattern)
        .arg("--no-heading")
        .arg("--line-number")
        .arg(root)
        .output()
        .context("Failed to run ripgrep - is it installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let matches: Vec<_> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(3, ':').collect();
            if parts.len() >= 3 {
                let path = parts[0].to_string();
                let line_num = parts[1].parse::<u64>().ok()?;
                let content = parts[2].to_string();
                Some((path, line_num, content))
            } else {
                None
            }
        })
        .collect();

    Ok(matches)
}

/// Benchmark helper
fn benchmark<F>(name: &str, iterations: usize, mut f: F) -> Duration
where
    F: FnMut() -> usize,
{
    println!("\n{}", "=".repeat(80));
    println!("{}", name);
    println!("{}", "=".repeat(80));

    let mut times = Vec::new();
    let mut match_count = 0;

    // Warmup
    let _ = f();

    for i in 0..iterations {
        let start = Instant::now();
        match_count = f();
        let elapsed = start.elapsed();
        times.push(elapsed);
        println!(
            "  Iteration {:2}: {:.2}ms ({} matches)",
            i + 1,
            elapsed.as_secs_f64() * 1000.0,
            match_count
        );
    }

    let avg = times.iter().sum::<Duration>() / iterations as u32;
    let min = times.iter().min().unwrap();
    let max = times.iter().max().unwrap();

    println!("\nResults:");
    println!("  Average: {:.2}ms", avg.as_secs_f64() * 1000.0);
    println!("  Min:     {:.2}ms", min.as_secs_f64() * 1000.0);
    println!("  Max:     {:.2}ms", max.as_secs_f64() * 1000.0);
    println!("  Matches: {}", match_count);

    avg
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        println!("Usage: {} <directory> [pattern] [iterations]", args[0]);
        println!("\nExample:");
        println!("  {} /path/to/codebase \"use std\" 10", args[0]);
        std::process::exit(1);
    }

    let root_dir = PathBuf::from(&args[1]);
    let pattern = if args.len() > 2 { &args[2] } else { "use" };
    let iterations = if args.len() > 3 {
        args[3].parse().unwrap_or(10)
    } else {
        10
    };

    if !root_dir.exists() {
        anyhow::bail!("Directory does not exist: {}", root_dir.display());
    }

    println!("\n{}", "=".repeat(80));
    println!("RIPGREP MEMORY-MAPPED SEARCH BENCHMARK");
    println!("{}", "=".repeat(80));
    println!("Directory:  {}", root_dir.display());
    println!("Pattern:    {}", pattern);
    println!("Iterations: {}", iterations);
    println!("{}", "=".repeat(80));

    // Build the cache (time this separately)
    println!("\nBuilding memory-mapped cache...");
    let cache_start = Instant::now();
    let cache = MmapCache::new(&root_dir)?;
    let cache_time = cache_start.elapsed();
    println!(
        "Cache built in {:.2}ms",
        cache_time.as_secs_f64() * 1000.0
    );

    // Benchmark 1: Memory-mapped search
    let mmap_avg = benchmark("Memory-Mapped Search", iterations, || {
        cache.search(pattern, false).unwrap().len()
    });

    // Benchmark 2: Subprocess ripgrep (if available)
    let rg_avg_opt = if Command::new("rg").arg("--version").output().is_ok() {
        Some(benchmark("Ripgrep Subprocess", iterations, || {
            ripgrep_subprocess(&root_dir, pattern).unwrap().len()
        }))
    } else {
        println!("\n{}", "=".repeat(80));
        println!("Ripgrep Subprocess");
        println!("{}", "=".repeat(80));
        println!("ripgrep not found in PATH - skipping subprocess benchmark");
        None
    };

    // Summary
    println!("\n\n{}", "=".repeat(80));
    println!("SUMMARY");
    println!("{}", "=".repeat(80));
    println!(
        "Cache build time:      {:.2}ms",
        cache_time.as_secs_f64() * 1000.0
    );
    println!("Files indexed:         {}", cache.files.len());
    println!(
        "Memory-mapped search:  {:.2}ms avg",
        mmap_avg.as_secs_f64() * 1000.0
    );

    if let Some(rg_avg) = rg_avg_opt {
        println!(
            "Subprocess ripgrep:    {:.2}ms avg",
            rg_avg.as_secs_f64() * 1000.0
        );
        let speedup = rg_avg.as_secs_f64() / mmap_avg.as_secs_f64();
        println!("\nSpeedup: {:.2}x faster", speedup);
        println!(
            "Time saved per search: {:.2}ms",
            (rg_avg.as_secs_f64() - mmap_avg.as_secs_f64()) * 1000.0
        );
    }

    println!("{}", "=".repeat(80));

    Ok(())
}
