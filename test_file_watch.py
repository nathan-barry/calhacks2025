#!/usr/bin/env python3
"""
Test file watching functionality for CURSERVE.

This test verifies that:
1. New files are automatically added to the cache
2. Modified files are automatically reloaded
3. Deleted files are automatically removed from the cache
"""

import time
import tempfile
import os
import shutil
from pathlib import Path
from curserve_client import MemSearchClient


def main():
    print("=" * 80)
    print("CURSERVE File Watch Test")
    print("=" * 80)
    print()

    # Create a temporary directory for testing
    with tempfile.TemporaryDirectory() as tmpdir:
        print(f"Test directory: {tmpdir}")
        print()

        # Create client
        client = MemSearchClient()

        try:
            # Step 1: Create initial test file
            print("[1] Creating initial test file...")
            test_file_1 = Path(tmpdir) / "test1.txt"
            test_file_1.write_text("Hello World\nFoo Bar\n")
            print(f"    Created: {test_file_1}")
            print()

            # Step 2: Allocate the directory
            print("[2] Allocating codebase...")
            client.alloc_pid(tmpdir)
            print("    ✓ Allocated (file watcher started synchronously)")
            print()

            # Step 3: Search for initial content
            print("[3] Searching for 'Hello'...")
            results = client.ripgrep("Hello")
            matches = [line for line in results.strip().split("\n") if line]
            print(f"    ✓ Found {len(matches)} match(es)")
            if matches:
                for match in matches:
                    print(f"      {match}")
            assert len(matches) == 1, f"Expected 1 match, got {len(matches)}"
            assert "Hello World" in results
            print("    ✓ Verification passed")
            print()

            # Step 4: Create a new file (test file watching for new files)
            print("[4] Creating new file (testing file watch - create)...")
            test_file_2 = Path(tmpdir) / "test2.txt"
            test_file_2.write_text("Hello Universe\nBaz Qux\n")
            print(f"    Created: {test_file_2}")

            # Give file watcher time to detect and load the file
            print("    Waiting for file watcher to detect new file...")
            time.sleep(2)

            # Search again
            results = client.ripgrep("Hello")
            matches = [line for line in results.strip().split("\n") if line]
            print(f"    ✓ Found {len(matches)} match(es)")
            if matches:
                for match in matches:
                    print(f"      {match}")
            assert len(matches) == 2, f"Expected 2 matches (both files), got {len(matches)}"
            assert "Hello World" in results
            assert "Hello Universe" in results
            print("    ✓ New file detected and indexed!")
            print()

            # Step 5: Modify existing file (test file watching for modifications)
            print("[5] Modifying existing file (testing file watch - modify)...")
            test_file_1.write_text("Hello World\nFoo Bar\nHello Galaxy\n")
            print(f"    Modified: {test_file_1}")

            # Give file watcher time to detect and reload the file
            print("    Waiting for file watcher to detect modification...")
            time.sleep(2)

            # Search again
            results = client.ripgrep("Hello")
            matches = [line for line in results.strip().split("\n") if line]
            print(f"    ✓ Found {len(matches)} match(es)")
            if matches:
                for match in matches:
                    print(f"      {match}")
            assert len(matches) == 3, f"Expected 3 matches (modified file has 2, new file has 1), got {len(matches)}"
            assert "Hello Galaxy" in results
            print("    ✓ Modified file detected and reloaded!")
            print()

            # Step 6: Delete a file (test file watching for deletions)
            print("[6] Deleting a file (testing file watch - delete)...")
            test_file_2.unlink()
            print(f"    Deleted: {test_file_2}")

            # Give file watcher time to detect the deletion
            print("    Waiting for file watcher to detect deletion...")
            time.sleep(2)

            # Search again
            results = client.ripgrep("Hello")
            matches = [line for line in results.strip().split("\n") if line]
            print(f"    ✓ Found {len(matches)} match(es)")
            if matches:
                for match in matches:
                    print(f"      {match}")
            assert len(matches) == 2, f"Expected 2 matches (only from test1.txt), got {len(matches)}"
            assert "Hello Universe" not in results, "Deleted file still appears in results!"
            print("    ✓ Deleted file removed from index!")
            print()

            # Step 7: Test pattern that no longer exists
            print("[7] Verifying deleted content is gone...")
            results = client.ripgrep("Universe")
            matches = [line for line in results.strip().split("\n") if line and line.strip()]
            print(f"    ✓ Found {len(matches)} match(es) for 'Universe'")
            assert len(matches) == 0, f"Expected 0 matches for deleted content, got {len(matches)}"
            print("    ✓ Deleted content not found (correct)")
            print()

            print("=" * 80)
            print("SUCCESS - All file watch tests passed!")
            print("=" * 80)
            print()
            print("Summary:")
            print("  ✓ New files are automatically detected and indexed")
            print("  ✓ Modified files are automatically reloaded")
            print("  ✓ Deleted files are automatically removed from index")
            print("  ✓ Search results reflect real-time file system state")
            print("=" * 80)

        except AssertionError as e:
            print()
            print("=" * 80)
            print("TEST FAILED")
            print("=" * 80)
            print(f"❌ Assertion Error: {e}")
            print()
            print("This likely means the file watcher is not working correctly.")
            print("Check that:")
            print("  1. The service is running with file watching enabled")
            print("  2. The file watcher thread started successfully")
            print("  3. There are no errors in the service logs")
            print("=" * 80)
            raise

        except Exception as e:
            print()
            print("=" * 80)
            print("ERROR")
            print("=" * 80)
            print(f"❌ {type(e).__name__}: {e}")
            import traceback
            traceback.print_exc()
            print("=" * 80)
            raise

        finally:
            client.close()


if __name__ == "__main__":
    main()
