use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use curserve::MmapCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
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

/// Shared state between threads
struct ServiceState {
    /// Mapping from PID to memory-mapped codebase
    codebases: HashMap<u32, MmapCache>,
    /// Mapping from PID to response socket stream
    response_sockets: HashMap<u32, UnixStream>,
    /// Mapping from PID to response socket listener (for accepting connections)
    response_listeners: HashMap<u32, UnixListener>,
}

impl ServiceState {
    fn new() -> Self {
        Self {
            codebases: HashMap::new(),
            response_sockets: HashMap::new(),
            response_listeners: HashMap::new(),
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

    // Create the MmapCache for this codebase
    match MmapCache::new(repo_path) {
        Ok(cache) => {
            state.codebases.insert(pid, cache);

            // Create response socket listener
            let response_socket_path = format!("/tmp/qwen_code_response_{}.sock", pid);

            // Remove old socket if it exists
            let _ = fs::remove_file(&response_socket_path);

            // Create the socket listener (but don't wait for connections here)
            let listener = UnixListener::bind(&response_socket_path)
                .context("Failed to bind response socket")?;

            state.response_listeners.insert(pid, listener);

            println!("[PID {}] Response socket created at {}", pid, response_socket_path);

            // Return success immediately - client will connect to response socket after receiving this response
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

/// Send response directly on a given stream (used for alloc_pid responses)
fn send_response_on_stream(stream: &mut UnixStream, response: &Response) -> Result<()> {
    let json = serde_json::to_string(response)?;
    stream.write_all(json.as_bytes())?;
    stream.write_all(b"\n")?; // Newline delimiter
    stream.flush()?;

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

/// Connection acceptor thread - accepts connections on response sockets
fn connection_acceptor(state: Arc<Mutex<ServiceState>>) -> Result<()> {
    println!("Connection acceptor thread started");

    loop {
        let mut state = state.lock().unwrap();

        // Check all listeners for pending connections
        let mut connections = Vec::new();
        for (&pid, listener) in &state.response_listeners {
            match listener.accept() {
                Ok((stream, _)) => {
                    println!("[PID {}] Client connected successfully", pid);
                    connections.push((pid, stream));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No pending connection, continue
                }
                Err(e) => {
                    eprintln!("[PID {}] Error accepting connection: {}", pid, e);
                }
            }
        }

        if !connections.is_empty() {
            println!("Accepted {} connections", connections.len());
        }

        // Apply connections to state
        for (pid, stream) in connections {
            state.response_sockets.insert(pid, stream);
            state.response_listeners.remove(&pid);
        }

        drop(state); // Release lock

        // No sleep - poll continuously for connections
        // This ensures we accept connections as soon as they arrive
    }
}

/// Main worker thread - processes requests from queue
fn request_worker(request_rx: Receiver<(Request, UnixStream)>, state: Arc<Mutex<ServiceState>>) -> Result<()> {
    println!("Worker thread started");

    loop {
        match request_rx.recv() {
            Ok((request, stream)) => {
                let mut state = state.lock().unwrap();

                let (pid, response, is_alloc_pid) = match &request {
                    Request::AllocPid { pid, repo_dir_path } => {
                        (*pid, handle_alloc_pid(&mut state, *pid, repo_dir_path.clone()), true)
                    }
                    Request::RequestRipgrep {
                        pid,
                        pattern,
                        case_sensitive,
                    } => (*pid, handle_ripgrep(&state, *pid, pattern.clone(), *case_sensitive), false),
                };

                match response {
                    Ok(resp) => {
                        if is_alloc_pid {
                            // For alloc_pid, send response directly on the request stream
                            if let Err(e) = send_response_on_stream(&mut stream.try_clone()?, &resp) {
                                eprintln!("[PID {}] Failed to send alloc_pid response: {}", pid, e);
                                // Clean up failed allocation
                                state.response_sockets.remove(&pid);
                                state.response_listeners.remove(&pid);
                                state.codebases.remove(&pid);
                            }
                        } else {
                            // For other requests, send on response socket
                            if let Err(e) = send_response(&mut state, pid, &resp) {
                                eprintln!("[PID {}] Failed to send response: {}", pid, e);
                                // Clean up dead socket
                                state.response_sockets.remove(&pid);
                                state.response_listeners.remove(&pid);
                                state.codebases.remove(&pid);
                            }
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

    Ok(())
}

fn main() -> Result<()> {
    println!("{}", "=".repeat(80));
    println!("CURSERVE Memory Search Service");
    println!("{}", "=".repeat(80));
    println!();

    // Create shared state
    let state = Arc::new(Mutex::new(ServiceState::new()));

    // Create channel for communication between listener and worker
    let (request_tx, request_rx) = bounded::<(Request, UnixStream)>(100);

    // Spawn connection acceptor thread
    let acceptor_state = Arc::clone(&state);
    let acceptor_thread = thread::spawn(move || {
        if let Err(e) = connection_acceptor(acceptor_state) {
            eprintln!("Connection acceptor error: {}", e);
        }
    });

    // Spawn listener thread
    let listener_tx = request_tx.clone();
    let listener_thread = thread::spawn(move || {
        if let Err(e) = request_listener(listener_tx) {
            eprintln!("Request listener error: {}", e);
        }
    });

    // Spawn worker thread
    let worker_state = Arc::clone(&state);
    let worker_thread = thread::spawn(move || {
        if let Err(e) = request_worker(request_rx, worker_state) {
            eprintln!("Request worker error: {}", e);
        }
    });

    println!("Service running. Press Ctrl+C to stop.");
    println!();

    // Wait for threads
    listener_thread.join().expect("Listener thread panicked");
    worker_thread.join().expect("Worker thread panicked");
    acceptor_thread.join().expect("Acceptor thread panicked");

    // Cleanup
    let _ = fs::remove_file(REQUEST_SOCKET);

    Ok(())
}
