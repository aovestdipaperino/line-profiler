# Quick Start Guide

This guide explains how to set up and use the line profiler after cloning the repository.

## Prerequisites

- Rust toolchain (rustup with nightly recommended)
- clang (for compiling instrumented LLVM IR)

## Building the Profiler

From the repository root, build all components:

```bash
# Build the profiler tools (release mode recommended)
cargo build --release

# Build the profiler runtime library
cd profiler-runtime && cargo build --release && cd ..
```

This produces the following binaries in `target/release/`:
- `cargo-profile-build` - builds your project with profiling instrumentation
- `cargo-profile-run` - builds and runs your project with profiling
- `rustc-wrapper` - internal wrapper used during compilation
- `lprf` - reads and displays `.lprf` profile files

## Installing as Cargo Subcommands

To use `cargo profile-build` and `cargo profile-run` from any directory, add the binaries to your PATH or copy them to `~/.cargo/bin/`:

```bash
cp target/release/cargo-profile-build ~/.cargo/bin/
cp target/release/cargo-profile-run ~/.cargo/bin/
cp target/release/rustc-wrapper ~/.cargo/bin/
cp target/release/lprf ~/.cargo/bin/
```

## Profiling Your Project

### Option 1: Build and Run Separately

```bash
# Build with profiling instrumentation
cargo profile-build

# Run the instrumented binary directly
./target/debug/your-binary

# View the profile results
lprf profile.lprf
```

### Option 2: Build and Run Together

```bash
# Build and run in one command
cargo profile-run

# View the profile results
lprf profile.lprf
```

## Common Options

Both `cargo profile-build` and `cargo profile-run` support standard cargo options:

```bash
# Build/run in release mode
cargo profile-build --release
cargo profile-run --release

# Build/run a specific binary
cargo profile-build --bin my-binary
cargo profile-run --bin my-binary

# Build/run an example
cargo profile-build --example my-example
cargo profile-run --example my-example
```

## Viewing Profile Results

The `lprf` command displays profile data in various formats:

```bash
# Default tree view
lprf profile.lprf

# Flat list sorted by time
lprf profile.lprf --format flat

# CSV output for analysis
lprf profile.lprf --format csv

# JSON output
lprf profile.lprf --format json

# Filter to show only entries above 1% of total time
lprf profile.lprf --min-percent 1

# Sort by self time instead of total time
lprf profile.lprf --sort self

# Include library code (hidden by default)
lprf profile.lprf --library=true
```

## Understanding the Output

The tree view shows a hierarchical breakdown of where time was spent:

```
└─ 77.1% myapp::main:0  0 calls, 2.16s total, 171µs self
  ├─ 77.1% main.rs:163 1 hits, 2.16s total
  │ ├─ 63.1% main.rs:183 1762 hits, 1.77s total
  │ ├─ 12.8% main.rs:183 5502 hits, 373ms total
```

Each line shows:
- Percentage of total time (inclusive)
- Function name or file:line location
- Hit count (how many times this location was executed)
- Total time (inclusive of children)
- Self time (exclusive, for functions only)

## Troubleshooting

**"rustc-wrapper not found"**: Ensure all binaries are in the same directory or properly installed to `~/.cargo/bin/`.

**No profile.lprf generated**: The profile is written when the program exits normally. If it crashes, no profile will be generated.

**clang errors**: Ensure clang is installed and accessible in your PATH. The profiler uses clang to compile the instrumented LLVM IR.
