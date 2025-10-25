use anyhow::{Context, Result};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use memmap2::Mmap;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Memory-mapped file cache for a single codebase
pub struct MmapCache {
    pub files: HashMap<PathBuf, Mmap>,
    pub root: PathBuf,
}

impl MmapCache {
    /// Create a new cache by memory-mapping all files in the given directory
    pub fn new(root: &Path) -> Result<Self> {
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

    /// Add or reload a single file in the cache
    pub fn reload_file(&mut self, path: &Path) -> Result<()> {
        // Skip if not a file
        if !path.is_file() {
            return Ok(());
        }

        // Skip binary files
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy();
            if matches!(
                ext_str.as_ref(),
                "png" | "jpg" | "jpeg" | "gif" | "pdf" | "zip" | "tar" | "gz" |
                "so" | "dylib" | "dll" | "exe" | "bin" | "o" | "a"
            ) {
                return Ok(());
            }
        }

        // Find and remove any existing entries for this file
        // (handles case where path format differs between initial load and file watcher)
        let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        let mut keys_to_remove = Vec::new();

        for key in self.files.keys() {
            let key_canonical = key.canonicalize().unwrap_or_else(|_| key.clone());
            if key_canonical == canonical_path {
                keys_to_remove.push(key.clone());
            }
        }

        for key in keys_to_remove {
            self.files.remove(&key);
        }

        match File::open(path) {
            Ok(file) => {
                let metadata = file.metadata()?;
                let file_size = metadata.len();

                // Skip very large files (>50MB) and empty files
                if file_size > 50 * 1024 * 1024 || file_size == 0 {
                    return Ok(());
                }

                match unsafe { Mmap::map(&file) } {
                    Ok(mmap) => {
                        println!("[FileWatch] Reloaded: {}", path.display());
                        self.files.insert(path.to_owned(), mmap);
                        Ok(())
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to mmap file: {}", e)),
                }
            }
            Err(e) => {
                // File was deleted, already removed above
                Err(anyhow::anyhow!("Failed to open file: {}", e))
            }
        }
    }

    /// Remove a file from the cache
    pub fn remove_file(&mut self, path: &Path) {
        // Find all entries that match this file canonically
        let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        let mut keys_to_remove = Vec::new();

        for key in self.files.keys() {
            let key_canonical = key.canonicalize().unwrap_or_else(|_| key.clone());
            if key_canonical == canonical_path {
                keys_to_remove.push(key.clone());
            }
        }

        for key in keys_to_remove {
            self.files.remove(&key);
            println!("[FileWatch] Removed: {}", key.display());
        }
    }

    /// Search all memory-mapped files for the given pattern
    pub fn search(&self, pattern: &str, case_sensitive: bool) -> Result<Vec<(String, u64, String)>> {
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
