use anyhow::{Context, Result};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use memmap2::Mmap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Represents a single search match
#[derive(Debug, Clone, Serialize)]
struct SearchMatch {
    path: String,
    line_number: u64,
    line: String,
    byte_offset: u64,
}

/// Search request parameters
#[derive(Debug, Deserialize)]
struct SearchRequest {
    pattern: String,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default = "default_max_results")]
    max_results: usize,
}

fn default_max_results() -> usize {
    1000
}

/// Search response
#[derive(Debug, Serialize)]
struct SearchResponse {
    matches: Vec<SearchMatch>,
    total_matches: usize,
    files_searched: usize,
    duration_ms: u128,
}

/// Memory-mapped file cache
struct MmapCache {
    files: HashMap<PathBuf, Mmap>,
    root: PathBuf,
}

impl MmapCache {
    /// Create a new cache by memory-mapping all files in the given directory
    fn new(root: &Path) -> Result<Self> {
        info!("Building memory-mapped cache for: {}", root.display());
        let mut files = HashMap::new();
        let mut file_count = 0;
        let mut total_bytes = 0u64;

        // Use ignore crate to walk directory, respecting .gitignore
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false) // Include hidden files
            .git_ignore(true) // Respect .gitignore
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let entry = entry.context("Failed to read directory entry")?;

            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();

            // Skip binary files heuristic - check file extension
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

                    // Skip very large files (>50MB) to avoid excessive memory usage
                    if file_size > 50 * 1024 * 1024 {
                        warn!("Skipping large file: {} ({} MB)", path.display(), file_size / 1024 / 1024);
                        continue;
                    }

                    // Skip empty files
                    if file_size == 0 {
                        continue;
                    }

                    match unsafe { Mmap::map(&file) } {
                        Ok(mmap) => {
                            file_count += 1;
                            total_bytes += file_size;
                            files.insert(path.to_owned(), mmap);
                        }
                        Err(e) => {
                            warn!("Failed to mmap {}: {}", path.display(), e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to open {}: {}", path.display(), e);
                }
            }
        }

        info!(
            "Memory-mapped {} files ({:.2} MB total)",
            file_count,
            total_bytes as f64 / 1024.0 / 1024.0
        );

        Ok(Self {
            files,
            root: root.to_owned(),
        })
    }

    /// Search all memory-mapped files for the given pattern
    fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        let start = std::time::Instant::now();

        // Build regex matcher
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!request.case_sensitive)
            .build(&request.pattern)
            .context("Invalid regex pattern")?;

        // Search all files in parallel using rayon
        let all_matches: Vec<Vec<SearchMatch>> = self
            .files
            .par_iter()
            .map(|(path, mmap)| {
                let mut matches = Vec::new();
                let mut searcher = Searcher::new();

                // Convert path to string relative to root
                let rel_path = path
                    .strip_prefix(&self.root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                // Search this file's memory-mapped contents
                let result = searcher.search_slice(
                    &matcher,
                    &mmap[..],
                    UTF8(|line_num, line| {
                        // Stop if we've hit the max results
                        if matches.len() >= request.max_results {
                            return Ok(false);
                        }

                        matches.push(SearchMatch {
                            path: rel_path.clone(),
                            line_number: line_num,
                            line: line.trim_end().to_string(),
                            byte_offset: 0, // We could calculate this if needed
                        });

                        Ok(true) // Continue searching
                    }),
                );

                if let Err(e) = result {
                    warn!("Search error in {}: {}", path.display(), e);
                }

                matches
            })
            .collect();

        // Flatten results and apply global max_results limit
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

/// Application state shared across requests
struct AppState {
    cache: Arc<RwLock<MmapCache>>,
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

/// Search endpoint
async fn search_handler(
    State(state): State<Arc<AppState>>,
    Query(request): Query<SearchRequest>,
) -> impl IntoResponse {
    let cache = state.cache.read().await;

    match cache.search(&request) {
        Ok(response) => {
            info!(
                "Search '{}' found {} matches in {}ms",
                request.pattern, response.total_matches, response.duration_ms
            );
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            warn!("Search error: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// POST search endpoint (for complex queries)
async fn search_post_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SearchRequest>,
) -> impl IntoResponse {
    let cache = state.cache.read().await;

    match cache.search(&request) {
        Ok(response) => {
            info!(
                "Search '{}' found {} matches in {}ms",
                request.pattern, response.total_matches, response.duration_ms
            );
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            warn!("Search error: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

/// Reload the cache
async fn reload_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    info!("Reloading cache...");
    let mut cache = state.cache.write().await;

    match MmapCache::new(&cache.root) {
        Ok(new_cache) => {
            *cache = new_cache;
            info!("Cache reloaded successfully");
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "reloaded",
                    "files": cache.files.len()
                })),
            )
        }
        Err(e) => {
            warn!("Failed to reload cache: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": e.to_string()
                })),
            )
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Get the directory to index from command line args
    let args: Vec<String> = std::env::args().collect();
    let root_dir = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        std::env::current_dir().context("Failed to get current directory")?
    };

    if !root_dir.exists() {
        anyhow::bail!("Directory does not exist: {}", root_dir.display());
    }

    // Build the memory-mapped cache
    let cache = MmapCache::new(&root_dir)?;

    let state = Arc::new(AppState {
        cache: Arc::new(RwLock::new(cache)),
    });

    // Build the router
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/search", get(search_handler))
        .route("/search", post(search_post_handler))
        .route("/reload", post(reload_handler))
        .with_state(state);

    // Start the server
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("Server listening on http://{}", addr);
    info!("Endpoints:");
    info!("  GET  /health - Health check");
    info!("  GET  /search?pattern=<regex>&case_sensitive=<bool>&max_results=<n> - Search");
    info!("  POST /search - Search (JSON body)");
    info!("  POST /reload - Reload file cache");

    axum::serve(listener, app)
        .await
        .context("Server error")?;

    Ok(())
}
