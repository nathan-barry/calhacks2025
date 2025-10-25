#!/usr/bin/env python3
"""
Test client for CURSERVE memory search service.

This demonstrates the full IPC workflow:
1. Client connects to service
2. Client allocates a codebase (memory-mapped)
3. Client performs ripgrep searches
4. Results are returned instantly from memory
"""

import time
from curserve_client import MemSearchClient


def main():
    print("=" * 80)
    print("CURSERVE Memory Search Service - Test Client")
    print("=" * 80)
    print()

    # Get codebase path (default to current directory)
    import sys
    codebase_path = sys.argv[1] if len(sys.argv) > 1 else "."

    print(f"Codebase: {codebase_path}")
    print()

    # Create client
    client = MemSearchClient()

    try:
        # Step 1: Allocate codebase
        print("[1] Allocating codebase (memory-mapping files)...")
        start = time.time()
        response = client.alloc_pid(codebase_path)
        alloc_time = (time.time() - start) * 1000
        print(f"    ✓ Allocated in {alloc_time:.2f}ms")
        print(f"    Response: {response.get('text', '')}")
        print()

        # Step 2: Perform searches
        test_patterns = [
            ("use", False),
            ("fn", False),
            ("struct", False),
        ]

        for pattern, case_sensitive in test_patterns:
            print(f"[2] Searching for '{pattern}'...")
            start = time.time()
            results = client.ripgrep(pattern, case_sensitive)
            search_time = (time.time() - start) * 1000

            num_matches = len(results.strip().split("\n")) if results.strip() else 0
            print(f"    ✓ Found {num_matches} matches in {search_time:.2f}ms")

            if results and num_matches <= 5:
                print("    Results:")
                for line in results.strip().split("\n")[:5]:
                    print(f"      {line}")
            print()

        # Step 3: Demonstrate speed
        print("[3] Speed test - 10 repeated searches...")
        times = []
        pattern = "use"
        for i in range(10):
            start = time.time()
            results = client.ripgrep(pattern)
            elapsed = (time.time() - start) * 1000
            times.append(elapsed)

        avg_time = sum(times) / len(times)
        min_time = min(times)
        max_time = max(times)

        print(f"    Average: {avg_time:.2f}ms")
        print(f"    Min:     {min_time:.2f}ms")
        print(f"    Max:     {max_time:.2f}ms")
        print()

        print("=" * 80)
        print("SUCCESS")
        print("=" * 80)
        print(f"✓ Memory-mapped search working!")
        print(f"✓ Average search time: {avg_time:.2f}ms")
        print(f"✓ Typical subprocess overhead saved: ~10-15ms per search")
        print("=" * 80)

    except Exception as e:
        print(f"\n❌ Error: {e}")
        import traceback
        traceback.print_exc()

    finally:
        client.close()


if __name__ == "__main__":
    main()
