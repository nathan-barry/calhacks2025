import mmap
import os
import re
from pathlib import Path
from typing import List, Tuple, Optional
import time


class InMemoryCodebase:
    """Fast in-memory codebase search using memory-mapped files"""

    def __init__(self, codebase_path: str):
        """
        Load codebase into memory-mapped files.

        Args:
            codebase_path: Path to the codebase directory
        """
        self.codebase_path = codebase_path
        self.files = {}  # filepath -> mmap object
        self.file_sizes = {}  # filepath -> size in bytes

        print(f"Loading codebase from: {codebase_path}")
        start = time.time()

        self._load_codebase()

        elapsed = time.time() - start
        total_size = sum(self.file_sizes.values()) / (1024 * 1024)  # MB
        print(f"Loaded {len(self.files)} files ({total_size:.1f} MB) in {elapsed:.2f}s")

    def _is_text_file(self, filepath: str) -> bool:
        """Check if file is a text file we should index"""
        # Common text file extensions
        text_extensions = {
            ".py",
            ".js",
            ".ts",
            ".jsx",
            ".tsx",
            ".java",
            ".c",
            ".cpp",
            ".h",
            ".hpp",
            ".go",
            ".rs",
            ".rb",
            ".php",
            ".cs",
            ".swift",
            ".kt",
            ".scala",
            ".r",
            ".html",
            ".css",
            ".scss",
            ".sass",
            ".less",
            ".json",
            ".yaml",
            ".yml",
            ".md",
            ".txt",
            ".xml",
            ".sql",
            ".sh",
            ".bash",
            ".zsh",
            ".fish",
            ".toml",
            ".ini",
            ".conf",
            ".config",
            ".env",
            ".proto",
            ".graphql",
            ".vue",
            ".svelte",
            ".elm",
            ".ex",
            ".exs",
            ".erl",
            ".hrl",
            ".clj",
            ".lua",
            ".pl",
            ".pm",
            ".raku",
            ".vim",
            ".el",
            ".lisp",
            ".scm",
            ".gradle",
            ".properties",
            ".dockerfile",
            ".makefile",
            ".cmake",
        }

        ext = Path(filepath).suffix.lower()
        if ext in text_extensions:
            return True

        # Also check common filenames without extensions
        filename = Path(filepath).name.lower()
        text_filenames = {
            "makefile",
            "dockerfile",
            "rakefile",
            "gemfile",
            "procfile",
            "readme",
            "license",
            "changelog",
            "contributing",
            "authors",
        }

        return filename in text_filenames

    def _should_skip_directory(self, dirname: str) -> bool:
        """Check if directory should be skipped"""
        skip_dirs = {
            ".git",
            ".svn",
            ".hg",
            ".bzr",  # Version control
            "node_modules",
            "bower_components",  # Node
            "__pycache__",
            ".pytest_cache",
            ".mypy_cache",  # Python
            "venv",
            ".venv",
            "env",
            ".env",
            "virtualenv",  # Python venvs
            "target",
            "build",
            "dist",
            "out",  # Build outputs
            ".idea",
            ".vscode",
            ".vs",  # IDEs
            "coverage",
            ".coverage",
            "htmlcov",  # Test coverage
            ".next",
            ".nuxt",
            ".cache",  # Framework caches
            "vendor",  # Dependencies
        }
        return dirname in skip_dirs

    def _load_codebase(self):
        """Walk directory tree and memory-map all text files"""
        for root, dirs, files in os.walk(self.codebase_path):
            # Filter out directories we want to skip (in-place)
            dirs[:] = [d for d in dirs if not self._should_skip_directory(d)]

            for filename in files:
                filepath = os.path.join(root, filename)

                # Only process text files
                if not self._is_text_file(filepath):
                    continue

                try:
                    self._mmap_file(filepath)
                except Exception as e:
                    # Skip files we can't read/mmap
                    print(f"Warning: Skipping {filepath}: {e}")

    def _mmap_file(self, filepath: str):
        """Memory-map a single file"""
        size = os.path.getsize(filepath)

        # Skip empty files
        if size == 0:
            return

        try:
            with open(filepath, "rb") as f:
                # Memory map the file (read-only)
                mmapped = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
                self.files[filepath] = mmapped
                self.file_sizes[filepath] = size
        except (OSError, ValueError) as e:
            # Some files can't be mmapped (e.g., pipes, special files)
            raise Exception(f"Cannot mmap: {e}")

    def grep(
        self,
        pattern: str,
        path_filter: Optional[str] = None,
        max_results: int = 1000,
        case_sensitive: bool = True,
    ) -> List[Tuple[str, int, str]]:
        """
        Search for pattern across all memory-mapped files.

        Args:
            pattern: Regex pattern to search for
            path_filter: Only search files containing this substring in their path
            max_results: Maximum number of results to return
            case_sensitive: Whether search is case-sensitive

        Returns:
            List of (filepath, line_number, line_content) tuples
        """
        # Compile regex pattern
        flags = 0 if case_sensitive else re.IGNORECASE
        try:
            # Use bytes pattern for searching bytes (mmap returns bytes)
            regex = re.compile(pattern.encode("utf-8"), flags)
        except re.error as e:
            print(f"Invalid regex pattern '{pattern}': {e}")
            return []

        results = []

        for filepath, mmap_obj in self.files.items():
            # Apply path filter if specified
            if path_filter and path_filter not in filepath:
                continue

            # Stop if we've hit max results
            if len(results) >= max_results:
                break

            try:
                # Search in memory-mapped file
                # Read entire file content as bytes
                content = mmap_obj[:]

                # Split into lines and search each line
                lines = content.split(b"\n")

                for line_num, line in enumerate(lines, start=1):
                    if regex.search(line):
                        try:
                            # Decode line to string (with error handling)
                            line_str = line.decode("utf-8", errors="replace").rstrip(
                                "\r\n"
                            )
                            results.append((filepath, line_num, line_str))

                            if len(results) >= max_results:
                                break
                        except Exception:
                            # Skip lines that cause issues
                            pass

            except Exception as e:
                print(f"Error searching {filepath}: {e}")
                continue

        return results

    def grep_formatted(
        self, pattern: str, path_filter: Optional[str] = None, max_results: int = 100
    ) -> str:
        """
        Grep with formatted output similar to ripgrep.

        Returns:
            Formatted string with results like: "filepath:line_num:line_content"
        """
        results = self.grep(pattern, path_filter, max_results)

        if not results:
            return f"No matches found for pattern: {pattern}"

        # Format results like ripgrep: filepath:line_number:content
        output_lines = []
        for filepath, line_num, content in results:
            # Make path relative to codebase_path for cleaner output
            try:
                rel_path = os.path.relpath(filepath, self.codebase_path)
            except ValueError:
                rel_path = filepath

            output_lines.append(f"{rel_path}:{line_num}:{content}")

        # Add summary
        summary = f"\n--- Found {len(results)} matches"
        if len(results) == max_results:
            summary += f" (limited to first {max_results})"
        summary += " ---"

        return "\n".join(output_lines) + summary

    def __del__(self):
        """Clean up memory-mapped files on deletion"""
        for mmap_obj in self.files.values():
            try:
                mmap_obj.close()
            except Exception:
                pass

    def get_stats(self) -> dict:
        """Get statistics about loaded codebase"""
        total_size = sum(self.file_sizes.values())
        return {
            "num_files": len(self.files),
            "total_size_bytes": total_size,
            "total_size_mb": total_size / (1024 * 1024),
            "avg_file_size_kb": (total_size / len(self.files) / 1024)
            if self.files
            else 0,
        }


def benchmark_fair():
    """Fair benchmark including one-time loading cost"""
    import subprocess

    codebase_path = "../nanochat/"
    search_pattern = "import"
    num_searches = 10  # Simulate 10 agent turns

    print("=" * 60)
    print("FAIR BENCHMARK: Including Setup Cost")
    print(f"Simulating {num_searches} searches (like agent turns)")
    print("=" * 60)

    # In-memory approach (load once, search many times)
    print("\n1. In-Memory Approach:")
    start = time.perf_counter()

    # One-time load
    mem_codebase = InMemoryCodebase(codebase_path)
    load_time = time.perf_counter() - start
    print(f"   Loading: {load_time * 1000:.2f}ms (one-time cost)")

    # Multiple searches
    search_start = time.perf_counter()
    for i in range(num_searches):
        results = mem_codebase.grep(search_pattern, max_results=1000)
    search_time = time.perf_counter() - search_start

    total_memory = (load_time + search_time) * 1000
    print(f"   {num_searches} searches: {search_time * 1000:.2f}ms")
    print(f"   Total: {total_memory:.2f}ms")

    # Subprocess approach (walk files every time)
    print("\n2. Subprocess Approach:")
    start = time.perf_counter()

    for i in range(num_searches):
        result = subprocess.run(
            ["rg", search_pattern, codebase_path],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if i == 0:
            print(f"   First grep: {(time.perf_counter() - start) * 1000:.2f}ms")

    total_subprocess = (time.perf_counter() - start) * 1000
    print(f"   {num_searches} searches: {total_subprocess:.2f}ms")
    print(f"   Total: {total_subprocess:.2f}ms")

    print("\n" + "=" * 60)
    print(f"In-Memory Total: {total_memory:.2f}ms")
    print(f"Subprocess Total: {total_subprocess:.2f}ms")
    print(f"")
    print(f"🚀 SPEEDUP: {total_subprocess / total_memory:.1f}x faster!")
    print("=" * 60)


def demo():
    """Simple demo of the grep functionality"""
    import sys

    if len(sys.argv) < 3:
        print("Usage: python script.py <codebase_path> <search_pattern>")
        print("Example: python script.py /path/to/react useState")
        sys.exit(1)

    codebase_path = sys.argv[1]
    pattern = sys.argv[2]

    # Load and search
    codebase = InMemoryCodebase(codebase_path)
    print(f"\nSearching for: {pattern}\n")

    results = codebase.grep_formatted(pattern, max_results=20)
    print(results)


if __name__ == "__main__":
    # Run demo
    # demo()

    # Or run benchmark:
    benchmark_fair()
