#!/usr/bin/env python3
"""
Example client for the ripgrep-mmap-server.
Demonstrates how to use the search API from Python.
"""

import requests
import json
import sys
import time


class SearchClient:
    def __init__(self, base_url="http://localhost:3000"):
        self.base_url = base_url

    def health_check(self):
        """Check if the server is running."""
        try:
            response = requests.get(f"{self.base_url}/health", timeout=2)
            return response.status_code == 200
        except requests.RequestException:
            return False

    def search(self, pattern, case_sensitive=False, max_results=1000):
        """
        Search for a pattern in the codebase.

        Args:
            pattern: Regex pattern to search for
            case_sensitive: Whether the search should be case-sensitive
            max_results: Maximum number of results to return

        Returns:
            dict with keys: matches, total_matches, files_searched, duration_ms
        """
        response = requests.post(
            f"{self.base_url}/search",
            json={
                "pattern": pattern,
                "case_sensitive": case_sensitive,
                "max_results": max_results,
            },
            timeout=30,
        )
        response.raise_for_status()
        return response.json()

    def reload(self):
        """Reload the file cache (e.g., after git pull)."""
        response = requests.post(f"{self.base_url}/reload", timeout=60)
        response.raise_for_status()
        return response.json()


def format_results(results):
    """Pretty-print search results."""
    print(f"\n{'='*80}")
    print(f"Found {results['total_matches']} matches in {results['files_searched']} files")
    print(f"Search took {results['duration_ms']}ms")
    print(f"{'='*80}\n")

    for match in results["matches"]:
        print(f"{match['path']}:{match['line_number']}")
        print(f"  {match['line']}")
        print()


def benchmark_search(client, pattern, iterations=10):
    """Benchmark search performance."""
    print(f"\nBenchmarking pattern: '{pattern}' ({iterations} iterations)")
    print("-" * 80)

    times = []
    for i in range(iterations):
        start = time.time()
        result = client.search(pattern, max_results=100)
        elapsed = (time.time() - start) * 1000  # Convert to ms

        times.append(elapsed)
        print(f"Iteration {i+1}: {elapsed:.2f}ms (server: {result['duration_ms']}ms)")

    print(f"\nAverage total time: {sum(times)/len(times):.2f}ms")
    print(f"Min: {min(times):.2f}ms, Max: {max(times):.2f}ms")
    print(f"Network overhead: ~{sum(times)/len(times) - result['duration_ms']:.2f}ms")


def main():
    client = SearchClient()

    # Check server health
    if not client.health_check():
        print("Error: Server is not running!")
        print("Start it with: cargo run -- /path/to/codebase")
        sys.exit(1)

    print("Server is running!")

    # Example 1: Simple search
    print("\n" + "="*80)
    print("Example 1: Search for 'fn ' (Rust functions)")
    print("="*80)
    results = client.search(r"fn\s+\w+", max_results=5)
    format_results(results)

    # Example 2: Case-sensitive search
    print("\n" + "="*80)
    print("Example 2: Case-sensitive search for 'TODO'")
    print("="*80)
    results = client.search("TODO", case_sensitive=True, max_results=10)
    format_results(results)

    # Example 3: Benchmark
    benchmark_search(client, "use", iterations=10)

    # Example 4: Complex regex
    print("\n" + "="*80)
    print("Example 4: Find struct definitions")
    print("="*80)
    results = client.search(r"struct\s+\w+", max_results=10)
    format_results(results)


if __name__ == "__main__":
    main()
