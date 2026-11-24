# line-profiler

[![GitHub stars](https://img.shields.io/github/stars/aovestdipaperino/line-profiler?logo=github&color=brightgreen)](https://github.com/aovestdipaperino/line-profiler/stargazers)

A line-level profiler for Rust that shows exactly where your program spends time, down to individual source lines.

## Features

- Line-by-line execution time tracking
- Hierarchical call tree visualization
- Integrates with Cargo as custom subcommands
- Multiple output formats (tree, flat, CSV, JSON, Mermaid)
- Filters to focus on hot code paths

## Installation

```bash
# Clone the repository
git clone https://github.com/aovestdipaperino/line-profiler
cd line-profiler

# Build everything
cargo build --release
cd profiler-runtime && cargo build --release && cd ..

# Install to cargo bin directory
cp target/release/cargo-profile-build ~/.cargo/bin/
cp target/release/cargo-profile-run ~/.cargo/bin/
cp target/release/rustc-wrapper ~/.cargo/bin/
cp target/release/lprf ~/.cargo/bin/
```

## Usage

Profile any Rust project:

```bash
# In your project directory
cargo profile-run

# View results
lprf profile.lprf
```

Example output:

```
└─ 77.1% myapp::main:0  0 calls, 2.16s total, 171µs self
  ├─ 63.1% main.rs:42 1762 hits, 1.77s total
  │ ├─ 45.2% main.rs:45 5000 hits, 1.27s total
  │ └─ 17.9% main.rs:48 5000 hits, 502ms total
  └─ 14.0% main.rs:55 100 hits, 393ms total
```

## Commands

### cargo profile-build

Builds your project with profiling instrumentation:

```bash
cargo profile-build           # Debug build
cargo profile-build --release # Release build
cargo profile-build --bin foo # Build specific binary
```

### cargo profile-run

Builds and runs with profiling:

```bash
cargo profile-run             # Build and run
cargo profile-run --release   # Release mode
cargo profile-run -- arg1 arg2  # Pass arguments to binary
```

### lprf

Reads and displays `.lprf` profile files:

```bash
lprf profile.lprf                    # Tree view (default)
lprf profile.lprf --format flat      # Flat list
lprf profile.lprf --format csv       # CSV output
lprf profile.lprf --format json      # JSON output
lprf profile.lprf --format mermaid   # Mermaid flowchart
lprf profile.lprf --min-percent 1    # Only show entries > 1%
lprf profile.lprf --sort self        # Sort by self time
```

## How It Works

The profiler instruments your code at the LLVM IR level:

1. Intercepts rustc compilation to emit LLVM IR
2. Injects profiling calls at function entry/exit and each source line
3. Links with a runtime library that records execution times
4. Writes results to a `.lprf` file when the program exits

This approach provides accurate line-level timing without requiring source code modifications.

## Requirements

- Rust (2024 edition)
- clang (for compiling instrumented IR)

## License

MIT
