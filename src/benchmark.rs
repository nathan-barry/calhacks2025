use anyhow::{Context, Result};
use curserve::MmapCache;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Run ripgrep as a subprocess for comparison
fn ripgrep_subprocess(root: &std::path::Path, pattern: &str) -> Result<Vec<(String, u64, String)>> {
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
