"""
CURSERVE Memory Search Service Client

Python client library for communicating with the mem-search-service daemon.
This can be used by qwen-code or any other Python process to perform
in-memory ripgrep operations on codebases.
"""

import json
import os
import socket
import threading


class MemSearchClient:
    """
    Client for communicating with the CURSERVE memory search service.

    Usage:
        client = MemSearchClient()
        client.alloc_pid("/path/to/codebase")
        results = client.ripgrep("pattern")
        client.close()
    """

    def __init__(self, pid=None):
        """
        Initialize the client.

        Args:
            pid: Process ID (defaults to current process)
        """
        self.pid = pid or os.getpid()
        self.request_socket_path = "/tmp/mem_search_service_requests.sock"
        self.response_socket_path = f"/tmp/qwen_code_response_{self.pid}.sock"
        self.request_socket = None
        self.response_socket = None
        self.response_listener = None
        self._allocated = False

    def _connect_request_socket(self):
        """Connect to the service's request socket."""
        if self.request_socket is None:
            self.request_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                self.request_socket.connect(self.request_socket_path)
            except FileNotFoundError:
                raise RuntimeError(
                    "Memory search service not running. "
                    "Start it with: cargo run --bin mem-search-service"
                )
            except ConnectionRefusedError:
                raise RuntimeError(
                    "Could not connect to memory search service. "
                    "Is it running?"
                )

    def _setup_response_socket(self):
        """Set up the response socket to receive responses from the service."""
        # Remove old socket if it exists
        try:
            os.remove(self.response_socket_path)
        except FileNotFoundError:
            pass

        # Create response socket and connect to service
        self.response_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)

        # The service creates the socket, we just connect to it
        # Wait a bit for the service to create it
        import time
        max_retries = 10
        for i in range(max_retries):
            try:
                self.response_socket.connect(self.response_socket_path)
                break
            except (FileNotFoundError, ConnectionRefusedError):
                if i == max_retries - 1:
                    raise RuntimeError(
                        f"Service did not create response socket at {self.response_socket_path}"
                    )
                time.sleep(0.1)

    def alloc_pid(self, repo_dir_path):
        """
        Allocate a codebase for this process.

        Args:
            repo_dir_path: Path to the repository/codebase directory

        Returns:
            Response dict with status and any messages

        Raises:
            RuntimeError: If allocation fails
        """
        if self._allocated:
            raise RuntimeError("PID already allocated. Call close() first to reallocate.")

        repo_dir_path = os.path.abspath(repo_dir_path)

        if not os.path.exists(repo_dir_path):
            raise ValueError(f"Repository path does not exist: {repo_dir_path}")

        # Connect to request socket
        self._connect_request_socket()

        # Send alloc_pid request
        request = {
            "type": "alloc_pid",
            "pid": self.pid,
            "repo_dir_path": repo_dir_path,
        }

        self.request_socket.sendall(json.dumps(request).encode() + b"\n")

        # Set up response socket (the service creates it after receiving alloc_pid)
        self._setup_response_socket()

        # Wait for response
        response = self._receive_response()

        if response["response_status"] == 1:
            self._allocated = True
            print(f"[CURSERVE] Allocated codebase: {repo_dir_path}")
            return response
        else:
            error = response.get("error", "Unknown error")
            raise RuntimeError(f"Failed to allocate codebase: {error}")

    def ripgrep(self, pattern, case_sensitive=False):
        """
        Search the allocated codebase for a pattern using ripgrep.

        Args:
            pattern: Regex pattern to search for
            case_sensitive: Whether the search should be case-sensitive

        Returns:
            Search results as a string (ripgrep format: path:line_num:content)

        Raises:
            RuntimeError: If no codebase is allocated or search fails
        """
        if not self._allocated:
            raise RuntimeError(
                "No codebase allocated. Call alloc_pid() first."
            )

        # Send ripgrep request
        request = {
            "type": "request_ripgrep",
            "pid": self.pid,
            "pattern": pattern,
            "case_sensitive": case_sensitive,
        }

        self.request_socket.sendall(json.dumps(request).encode() + b"\n")

        # Wait for response
        response = self._receive_response()

        if response["response_status"] == 1:
            return response.get("text", "")
        else:
            error = response.get("error", "Unknown error")
            raise RuntimeError(f"Search failed: {error}")

    def _receive_response(self):
        """Receive and parse a response from the response socket."""
        # Read until newline
        buffer = b""
        while True:
            chunk = self.response_socket.recv(4096)
            if not chunk:
                raise RuntimeError("Connection closed by service")
            buffer += chunk
            if b"\n" in buffer:
                break

        # Parse JSON
        json_str = buffer.decode().strip()
        return json.loads(json_str)

    def close(self):
        """Close all connections and clean up."""
        if self.request_socket:
            self.request_socket.close()
            self.request_socket = None

        if self.response_socket:
            self.response_socket.close()
            self.response_socket = None

        # Clean up response socket file
        try:
            os.remove(self.response_socket_path)
        except FileNotFoundError:
            pass

        self._allocated = False

    def __enter__(self):
        """Context manager entry."""
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit."""
        self.close()


def ripgrep(pattern, codebase_path=None, case_sensitive=False):
    """
    Convenience function for one-off ripgrep operations.

    Args:
        pattern: Regex pattern to search for
        codebase_path: Path to codebase (defaults to current directory)
        case_sensitive: Whether the search should be case-sensitive

    Returns:
        Search results as a string
    """
    if codebase_path is None:
        codebase_path = os.getcwd()

    with MemSearchClient() as client:
        client.alloc_pid(codebase_path)
        return client.ripgrep(pattern, case_sensitive)


if __name__ == "__main__":
    # Example usage
    import sys

    if len(sys.argv) < 3:
        print("Usage: python curserve_client.py <codebase_path> <pattern>")
        print("\nExample:")
        print("  python curserve_client.py /path/to/repo 'use std'")
        sys.exit(1)

    codebase = sys.argv[1]
    pattern = sys.argv[2]

    print(f"Searching for '{pattern}' in {codebase}")
    print("=" * 80)

    results = ripgrep(pattern, codebase)

    if results:
        print(results)
        print("=" * 80)
        num_matches = len(results.strip().split("\n")) if results.strip() else 0
        print(f"Found {num_matches} matches")
    else:
        print("No matches found")
