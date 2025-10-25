use anyhow::{Context, Result};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use memmap2::Mmap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchMatch {
    path: String,
    line_number: u64,
    line: String,
}

#[derive(Debug, Deserialize)]
struct SearchRequest {
    pattern: String,
    case_sensitive: bool,
    max_results: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchResponse {
    matches: Vec<SearchMatch>,
    total_matches: usize,
    files_searched: usize,
    duration_ms: u128,
}

struct MmapCache {
    files: HashMap<PathBuf, Mmap>,
    root: PathBuf,
}

impl MmapCache {
    fn new(root: &Path) -> Result<Self> {
        println!("Building memory-mapped cache for: {}", root.display());
        let mut files = HashMap::new();
        let mut file_count = 0;
        let mut total_bytes = 0u64;

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
                    "png" | "jpg" | "jpeg" | "gif" | "pdf" | "zip" | "tar" | "gz" | "so" | "dylib" | "dll" | "exe" | "bin" | "o" | "a"
                ) {
                    continue;
                }
            }

            match File::open(path) {
                Ok(file) => {
                    let metadata = file.metadata()?;
                    let file_size = metadata.len();

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
            "Indexed {} files ({:.2} MB total)",
            file_count,
            total_bytes as f64 / 1024.0 / 1024.0
        );

        Ok(Self {
            files,
            root: root.to_owned(),
        })
    }

    fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        let start = Instant::now();

        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!request.case_sensitive)
            .build(&request.pattern)
            .context("Invalid regex pattern")?;

        let all_matches: Vec<Vec<SearchMatch>> = self
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
                        if matches.len() >= request.max_results {
                            return Ok(false);
                        }

                        matches.push(SearchMatch {
                            path: rel_path.clone(),
                            line_number: line_num,
                            line: line.trim_end().to_string(),
                        });

                        Ok(true)
                    }),
                );

                matches
            })
            .collect();

        let mut matches: Vec<SearchMatch> = all_matches.into_iter().flatten().collect();
        let total_matches = matches.len();
        matches.truncate(request.max_results);

        let duration = start.elapsed();

        Ok(SearchResponse {
            matches,
            total_matches,
            files_searched: self.files.len(),
            duration_ms: duration.as_millis(),
        })
    }
}

struct BenchmarkStats {
    times: Vec<Duration>,
    total_matches: usize,
}

impl BenchmarkStats {
    fn new() -> Self {
        Self {
            times: Vec::new(),
            total_matches: 0,
        }
    }

    fn add(&mut self, duration: Duration, matches: usize) {
        self.times.push(duration);
        self.total_matches = matches;
    }

    fn print(&self, name: &str) {
        if self.times.is_empty() {
            println!("No results for {}", name);
            return;
        }

        let times_ms: Vec<f64> = self.times.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        let avg = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
        let min = times_ms.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max = times_ms.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));

        // Calculate standard deviation
        let variance = times_ms.iter().map(|t| (t - avg).powi(2)).sum::<f64>() / times_ms.len() as f64;
        let std_dev = variance.sqrt();

        println!("\n{}", "=".repeat(80));
        println!("{}", name);
        println!("{}", "=".repeat(80));
        println!("Iterations:      {}", self.times.len());
        println!("Total matches:   {}", self.total_matches);
        println!("Average time:    {:.2}ms", avg);
        println!("Min time:        {:.2}ms", min);
        println!("Max time:        {:.2}ms", max);
        println!("Std deviation:   {:.2}ms", std_dev);
        println!("Median time:     {:.2}ms", median(&times_ms));
        println!("{}", "=".repeat(80));
    }
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn benchmark_mmap_search(cache: &MmapCache, pattern: &str, iterations: usize) -> BenchmarkStats {
    println!("\nBenchmarking in-memory search: '{}'", pattern);
    println!("{}", "-".repeat(80));

    let mut stats = BenchmarkStats::new();
    let request = SearchRequest {
        pattern: pattern.to_string(),
        case_sensitive: false,
        max_results: 1000,
    };

    // Warmup
    let _ = cache.search(&request);

    for i in 0..iterations {
        let start = Instant::now();
        let result = cache.search(&request).unwrap();
        let elapsed = start.elapsed();

        stats.add(elapsed, result.total_matches);
        println!(
            "  Iteration {:2}: {:.2}ms ({} matches)",
            i + 1,
            elapsed.as_secs_f64() * 1000.0,
            result.total_matches
        );
    }

    stats
}

fn benchmark_ripgrep(root: &Path, pattern: &str, iterations: usize) -> Option<BenchmarkStats> {
    println!("\nBenchmarking ripgrep subprocess: '{}'", pattern);
    println!("{}", "-".repeat(80));

    let mut stats = BenchmarkStats::new();

    // Check if rg is available
    if Command::new("rg").arg("--version").output().is_err() {
        println!("  ripgrep (rg) not found in PATH, skipping comparison");
        return None;
    }

    // Warmup
    let _ = Command::new("rg")
        .arg(pattern)
        .arg("--max-count=1000")
        .arg("--no-heading")
        .arg(root)
        .output();

    for i in 0..iterations {
        let start = Instant::now();
        let output = Command::new("rg")
            .arg(pattern)
            .arg("--max-count=1000")
            .arg("--no-heading")
            .arg(root)
            .output()
            .unwrap();
        let elapsed = start.elapsed();

        let match_count = output.stdout.iter().filter(|&&b| b == b'\n').count();
        stats.add(elapsed, match_count);
        println!(
            "  Iteration {:2}: {:.2}ms ({} matches)",
            i + 1,
            elapsed.as_secs_f64() * 1000.0,
            match_count
        );
    }

    Some(stats)
}

fn benchmark_http_api(pattern: &str, iterations: usize, port: u16) -> Option<BenchmarkStats> {
    println!("\nBenchmarking HTTP API: '{}'", pattern);
    println!("{}", "-".repeat(80));

    let client = reqwest::blocking::Client::new();
    let url = format!("http://localhost:{}/search", port);

    // Check if server is running
    if client
        .get(format!("http://localhost:{}/health", port))
        .timeout(Duration::from_secs(1))
        .send()
        .is_err()
    {
        println!("  Server not running on port {}, skipping HTTP benchmark", port);
        println!("  Start server with: cargo run -- <directory>");
        return None;
    }

    let mut stats = BenchmarkStats::new();

    // Warmup
    let _ = client
        .post(&url)
        .json(&serde_json::json!({
            "pattern": pattern,
            "case_sensitive": false,
            "max_results": 1000
        }))
        .send();

    for i in 0..iterations {
        let start = Instant::now();
        let response = client
            .post(&url)
            .json(&serde_json::json!({
                "pattern": pattern,
                "case_sensitive": false,
                "max_results": 1000
            }))
            .send()
            .unwrap();

        let elapsed = start.elapsed();
        let result: SearchResponse = response.json().unwrap();

        stats.add(elapsed, result.total_matches);
        println!(
            "  Iteration {:2}: {:.2}ms (server: {}ms, network overhead: {:.2}ms, {} matches)",
            i + 1,
            elapsed.as_secs_f64() * 1000.0,
            result.duration_ms,
            elapsed.as_secs_f64() * 1000.0 - result.duration_ms as f64,
            result.total_matches
        );
    }

    Some(stats)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: {} <directory> [pattern] [iterations]", args[0]);
        println!("\nExamples:");
        println!("  {} /path/to/codebase", args[0]);
        println!("  {} /path/to/codebase 'fn ' 20", args[0]);
        println!("  {} . 'use std' 10", args[0]);
        std::process::exit(1);
    }

    let root_dir = PathBuf::from(&args[1]);
    let pattern = if args.len() > 2 {
        &args[2]
    } else {
        "use"
    };
    let iterations = if args.len() > 3 {
        args[3].parse().unwrap_or(10)
    } else {
        10
    };

    println!("\n{}", "=".repeat(80));
    println!("RIPGREP MEMORY-MAPPED SEARCH BENCHMARK");
    println!("{}", "=".repeat(80));
    println!("Directory:  {}", root_dir.display());
    println!("Pattern:    {}", pattern);
    println!("Iterations: {}", iterations);
    println!("{}", "=".repeat(80));

    // Build cache and time it
    println!("\n{}", "=".repeat(80));
    println!("CACHE BUILD TIME");
    println!("{}", "=".repeat(80));
    let cache_start = Instant::now();
    let cache = MmapCache::new(&root_dir)?;
    let cache_duration = cache_start.elapsed();
    println!("Cache build time: {:.2}ms", cache_duration.as_secs_f64() * 1000.0);
    println!("{}", "=".repeat(80));

    // Benchmark in-memory search
    let mmap_stats = benchmark_mmap_search(&cache, pattern, iterations);

    // Benchmark ripgrep subprocess
    let rg_stats = benchmark_ripgrep(&root_dir, pattern, iterations);

    // Benchmark HTTP API (if server is running)
    let http_stats = benchmark_http_api(pattern, iterations, 3000);

    // Print results
    println!("\n\n{}", "=".repeat(80));
    println!("BENCHMARK RESULTS");
    println!("{}", "=".repeat(80));

    mmap_stats.print("In-Memory Search (Direct)");

    if let Some(stats) = rg_stats {
        stats.print("Ripgrep Subprocess (Baseline)");

        // Calculate speedup
        let mmap_avg = mmap_stats.times.iter().sum::<Duration>().as_secs_f64()
            / mmap_stats.times.len() as f64;
        let rg_avg =
            stats.times.iter().sum::<Duration>().as_secs_f64() / stats.times.len() as f64;
        let speedup = rg_avg / mmap_avg;

        println!("\n{}", "=".repeat(80));
        println!("SPEEDUP ANALYSIS");
        println!("{}", "=".repeat(80));
        println!("Memory-mapped vs Ripgrep subprocess: {:.2}x faster", speedup);
        println!("Time saved per search: {:.2}ms", (rg_avg - mmap_avg) * 1000.0);
        println!("{}", "=".repeat(80));
    }

    if let Some(stats) = http_stats {
        stats.print("HTTP API (Client -> Server)");
    }

    println!("\n{}", "=".repeat(80));
    println!("SUMMARY");
    println!("{}", "=".repeat(80));
    println!("✓ Cache build time: {:.2}ms", cache_duration.as_secs_f64() * 1000.0);
    println!("✓ Files indexed: {}", cache.files.len());
    println!(
        "✓ In-memory search: {:.2}ms avg",
        mmap_stats.times.iter().sum::<Duration>().as_secs_f64() / mmap_stats.times.len() as f64
            * 1000.0
    );
    println!("{}", "=".repeat(80));

    Ok(())
}
