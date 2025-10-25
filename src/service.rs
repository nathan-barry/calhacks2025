use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use curserve::MmapCache;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

const REQUEST_SOCKET: &str = "/tmp/mem_search_service_requests.sock";

/// Request types from clients
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Request {
    #[serde(rename = "alloc_pid")]
    AllocPid {
        pid: u32,
        repo_dir_path: String,
    },
    #[serde(rename = "request_ripgrep")]
    RequestRipgrep {
        pid: u32,
        pattern: String,
        #[serde(default)]
        case_sensitive: bool,
    },
}

/// Response types sent back to clients
#[derive(Debug, Serialize)]
struct Response {
    response_status: u8, // 1 = success, 0 = failure
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Response {
    fn success(text: Option<String>) -> Self {
        Self {
            response_status: 1,
            text,
            error: None,
        }
    }

    fn failure(error: String) -> Self {
        Self {
            response_status: 0,
            text: None,
            error: Some(error),
        }
    }
}

/// File watcher events sent to worker thread
#[derive(Debug)]
enum FileWatchEvent {
    /// A file was created or modified
    FileChanged { pid: u32, path: PathBuf },
    /// A file was deleted
    FileDeleted { pid: u32, path: PathBuf },
}

/// Commands sent to file watcher thread
#[derive(Debug)]
enum WatchCommand {
    /// Start watching a directory for a PID
    Watch { pid: u32, path: PathBuf },
    /// Stop watching a PID's directory
    Unwatch { pid: u32 },
}

/// Shared state between threads
struct ServiceState {
    /// Mapping from PID to memory-mapped codebase
    codebases: HashMap<u32, MmapCache>,
    /// Mapping from PID to response socket stream
    response_sockets: HashMap<u32, UnixStream>,
    /// Channel to send watch commands to file watcher thread
    watch_cmd_tx: Sender<WatchCommand>,
}

impl ServiceState {
    fn new(watch_cmd_tx: Sender<WatchCommand>) -> Self {
        Self {
            codebases: HashMap::new(),
            response_sockets: HashMap::new(),
            watch_cmd_tx,
        }
    }
}

/// Handle alloc_pid request
fn handle_alloc_pid(
    state: &mut ServiceState,
    pid: u32,
    repo_dir_path: String,
) -> Result<Response> {
    let repo_path = Path::new(&repo_dir_path);

    if !repo_path.exists() {
        return Ok(Response::failure(format!(
            "Repository path does not exist: {}",
            repo_dir_path
        )));
    }

    println!("[PID {}] Allocating codebase: {}", pid, repo_dir_path);

    // Canonicalize the path to resolve symlinks (important on macOS where /var -> /private/var)
    let canonical_path = repo_path.canonicalize()
        .unwrap_or_else(|_| repo_path.to_owned());

    // Create the MmapCache for this codebase
    match MmapCache::new(repo_path) {
        Ok(cache) => {
            state.codebases.insert(pid, cache);

            // Send watch command to file watcher thread (synchronous - watcher will start immediately)
            println!("[PID {}] Canonical path for watching: {}", pid, canonical_path.display());
            if let Err(e) = state.watch_cmd_tx.send(WatchCommand::Watch {
                pid,
                path: canonical_path.clone()
            }) {
                eprintln!("[PID {}] Failed to send watch command: {}", pid, e);
            }

            // Create and connect to response socket
            let response_socket_path = format!("/tmp/qwen_code_response_{}.sock", pid);

            // Remove old socket if it exists
            let _ = fs::remove_file(&response_socket_path);

            // Create the socket
            let listener = UnixListener::bind(&response_socket_path)
                .context("Failed to bind response socket")?;

            println!("[PID {}] Waiting for client to connect to {}", pid, response_socket_path);

            // Wait for the client to connect (blocking)
            let (stream, _) = listener.accept()
                .context("Failed to accept connection on response socket")?;

            state.response_sockets.insert(pid, stream);

            println!("[PID {}] Client connected successfully", pid);

            Ok(Response::success(Some(format!(
                "Allocated {} files",
                state.codebases.get(&pid).unwrap().files.len()
            ))))
        }
        Err(e) => Ok(Response::failure(format!(
            "Failed to load codebase: {}",
            e
        ))),
    }
}

/// Handle request_ripgrep request
fn handle_ripgrep(
    state: &ServiceState,
    pid: u32,
    pattern: String,
    case_sensitive: bool,
) -> Result<Response> {
    // Check if PID has an allocated codebase
    let cache = match state.codebases.get(&pid) {
        Some(c) => c,
        None => {
            return Ok(Response::failure(format!(
                "PID {} has no allocated codebase. Call alloc_pid first.",
                pid
            )))
        }
    };

    println!("[PID {}] Searching for pattern: {}", pid, pattern);

    // Perform the search
    match cache.search(&pattern, case_sensitive) {
        Ok(matches) => {
            // Format output like ripgrep: path:line_num:content
            let output = matches
                .iter()
                .map(|(path, line_num, content)| format!("{}:{}:{}", path, line_num, content))
                .collect::<Vec<_>>()
                .join("\n");

            println!("[PID {}] Found {} matches", pid, matches.len());
            Ok(Response::success(Some(output)))
        }
        Err(e) => Ok(Response::failure(format!("Search failed: {}", e))),
    }
}

/// Send response to client's response socket
fn send_response(state: &mut ServiceState, pid: u32, response: &Response) -> Result<()> {
    let socket = state.response_sockets.get_mut(&pid).context(format!(
        "No response socket for PID {}",
        pid
    ))?;

    let json = serde_json::to_string(response)?;
    socket.write_all(json.as_bytes())?;
    socket.write_all(b"\n")?; // Newline delimiter
    socket.flush()?;

    Ok(())
}

/// Request listener thread - receives requests and adds to queue
fn request_listener(request_tx: Sender<(Request, UnixStream)>) -> Result<()> {
    // Remove old socket if it exists
    let _ = fs::remove_file(REQUEST_SOCKET);

    let listener =
        UnixListener::bind(REQUEST_SOCKET).context("Failed to bind request socket")?;

    println!("Request listener started on {}", REQUEST_SOCKET);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // Read request from stream
                let reader = BufReader::new(stream.try_clone()?);
                for line in reader.lines() {
                    match line {
                        Ok(json_str) => {
                            match serde_json::from_str::<Request>(&json_str) {
                                Ok(request) => {
                                    println!("Received request: {:?}", request);
                                    if let Err(e) = request_tx.send((request, stream.try_clone()?)) {
                                        eprintln!("Failed to send request to worker: {}", e);
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Failed to parse request: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading line: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Connection error: {}", e);
            }
        }
    }

    Ok(())
}

/// Main worker thread - processes requests and file watch events
fn request_worker(
    request_rx: Receiver<(Request, UnixStream)>,
    filewatch_rx: Receiver<FileWatchEvent>,
    state: Arc<Mutex<ServiceState>>,
) -> Result<()> {
    println!("Worker thread started");

    loop {
        crossbeam_channel::select! {
            recv(request_rx) -> msg => {
                match msg {
                    Ok((request, _stream)) => {
                        let mut state = state.lock().unwrap();

                        let (pid, response) = match request {
                            Request::AllocPid { pid, repo_dir_path } => {
                                (pid, handle_alloc_pid(&mut state, pid, repo_dir_path))
                            }
                            Request::RequestRipgrep {
                                pid,
                                pattern,
                                case_sensitive,
                            } => (pid, handle_ripgrep(&state, pid, pattern, case_sensitive)),
                        };

                        match response {
                            Ok(resp) => {
                                if let Err(e) = send_response(&mut state, pid, &resp) {
                                    eprintln!("[PID {}] Failed to send response: {}", pid, e);
                                    // Clean up dead socket
                                    state.response_sockets.remove(&pid);
                                    state.codebases.remove(&pid);
                                    let _ = state.watch_cmd_tx.send(WatchCommand::Unwatch { pid });
                                }
                            }
                            Err(e) => {
                                eprintln!("[PID {}] Request handler error: {}", pid, e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Channel receive error: {}", e);
                        break;
                    }
                }
            }
            recv(filewatch_rx) -> event => {
                match event {
                    Ok(FileWatchEvent::FileChanged { pid, path }) => {
                        println!("[Worker] Received FileChanged for PID {}: {}", pid, path.display());
                        let mut state = state.lock().unwrap();
                        if let Some(cache) = state.codebases.get_mut(&pid) {
                            if let Err(e) = cache.reload_file(&path) {
                                eprintln!("[Worker] Failed to reload file: {}", e);
                            }
                        } else {
                            eprintln!("[Worker] No cache found for PID {}", pid);
                        }
                    }
                    Ok(FileWatchEvent::FileDeleted { pid, path }) => {
                        println!("[Worker] Received FileDeleted for PID {}: {}", pid, path.display());
                        let mut state = state.lock().unwrap();
                        if let Some(cache) = state.codebases.get_mut(&pid) {
                            cache.remove_file(&path);
                        }
                    }
                    Err(_) => {
                        // File watcher channel closed
                    }
                }
            }
        }
    }

    Ok(())
}

/// File watcher thread - monitors codebase directories for changes
fn file_watcher(
    filewatch_tx: Sender<FileWatchEvent>,
    watch_cmd_rx: Receiver<WatchCommand>,
) -> Result<()> {
    use std::time::Duration;

    println!("File watcher thread started");

    // Create a channel for notify events (std mpsc)
    let (notify_tx_std, notify_rx_std) = std::sync::mpsc::channel();

    // Create a crossbeam channel to bridge notify events
    let (notify_tx, notify_rx) = bounded::<Result<Event, notify::Error>>(100);

    // Spawn bridge thread to convert std mpsc to crossbeam
    let bridge_thread = thread::spawn(move || {
        while let Ok(event) = notify_rx_std.recv() {
            if notify_tx.send(event).is_err() {
                break;
            }
        }
    });

    // Create the file watcher
    let mut watcher: RecommendedWatcher = Watcher::new(
        notify_tx_std,
        notify::Config::default().with_poll_interval(Duration::from_secs(1)),
    )?;

    // Track which paths we're watching
    let mut watched_paths: HashMap<PathBuf, u32> = HashMap::new();

    loop {
        crossbeam_channel::select! {
            // Handle watch commands (synchronous - no race condition)
            recv(watch_cmd_rx) -> cmd => {
                match cmd {
                    Ok(WatchCommand::Watch { pid, path }) => {
                        if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
                            eprintln!("[PID {}] Failed to watch {}: {}", pid, path.display(), e);
                        } else {
                            println!("[PID {}] Watching directory: {}", pid, path.display());
                            watched_paths.insert(path, pid);
                        }
                    }
                    Ok(WatchCommand::Unwatch { pid }) => {
                        // Remove all paths for this PID
                        watched_paths.retain(|path, p| {
                            if *p == pid {
                                let _ = watcher.unwatch(path);
                                println!("[PID {}] Unwatching directory: {}", pid, path.display());
                                false
                            } else {
                                true
                            }
                        });
                    }
                    Err(_) => {
                        eprintln!("Watch command channel disconnected");
                        break;
                    }
                }
            }
            // Handle file system events from notify (via bridge)
            recv(notify_rx) -> event => {
                match event {
                    Ok(Ok(event)) => {
                        println!("[FileWatch] Event: {:?}", event);
                        if let Err(e) = handle_notify_event(&event, &watched_paths, &filewatch_tx) {
                            eprintln!("Error handling file event: {}", e);
                        }
                    }
                    Ok(Err(e)) => {
                        eprintln!("File watcher error: {}", e);
                    }
                    Err(_) => {
                        eprintln!("Notify channel disconnected");
                        break;
                    }
                }
            }
        }
    }

    // Clean up bridge thread
    drop(notify_rx);
    let _ = bridge_thread.join();

    Ok(())
}

/// Handle a notify event and send appropriate FileWatchEvent
fn handle_notify_event(
    event: &Event,
    watched_paths: &HashMap<PathBuf, u32>,
    filewatch_tx: &Sender<FileWatchEvent>,
) -> Result<()> {
    use notify::EventKind::*;

    for path in &event.paths {
        // Find which PID this path belongs to
        let pid = watched_paths
            .iter()
            .find(|(root, _)| path.starts_with(root))
            .map(|(_, pid)| *pid);

        if let Some(pid) = pid {
            match event.kind {
                Create(_) | Modify(_) => {
                    println!("[FileWatch] Sending FileChanged event for PID {}: {}", pid, path.display());
                    filewatch_tx.send(FileWatchEvent::FileChanged {
                        pid,
                        path: path.clone(),
                    })?;
                }
                Remove(_) => {
                    println!("[FileWatch] Sending FileDeleted event for PID {}: {}", pid, path.display());
                    filewatch_tx.send(FileWatchEvent::FileDeleted {
                        pid,
                        path: path.clone(),
                    })?;
                }
                _ => {}
            }
        } else {
            println!("[FileWatch] Path {} does not belong to any watched PID", path.display());
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    println!("{}", "=".repeat(80));
    println!("CURSERVE Memory Search Service");
    println!("{}", "=".repeat(80));
    println!();

    // Create channels
    let (request_tx, request_rx) = bounded::<(Request, UnixStream)>(100);
    let (filewatch_tx, filewatch_rx) = bounded::<FileWatchEvent>(100);
    let (watch_cmd_tx, watch_cmd_rx) = bounded::<WatchCommand>(100);

    // Create shared state (with watch command sender)
    let state = Arc::new(Mutex::new(ServiceState::new(watch_cmd_tx)));

    // Spawn listener thread
    let listener_tx = request_tx.clone();
    let listener_thread = thread::spawn(move || {
        if let Err(e) = request_listener(listener_tx) {
            eprintln!("Request listener error: {}", e);
        }
    });

    // Spawn worker thread
    let worker_state = state.clone();
    let worker_thread = thread::spawn(move || {
        if let Err(e) = request_worker(request_rx, filewatch_rx, worker_state) {
            eprintln!("Request worker error: {}", e);
        }
    });

    // Spawn file watcher thread (receives watch commands, no longer needs state)
    let watcher_thread = thread::spawn(move || {
        if let Err(e) = file_watcher(filewatch_tx, watch_cmd_rx) {
            eprintln!("File watcher error: {}", e);
        }
    });

    println!("Service running. Press Ctrl+C to stop.");
    println!();

    // Wait for threads
    listener_thread.join().expect("Listener thread panicked");
    worker_thread.join().expect("Worker thread panicked");
    watcher_thread.join().expect("Watcher thread panicked");

    // Cleanup
    let _ = fs::remove_file(REQUEST_SOCKET);

    Ok(())
}
