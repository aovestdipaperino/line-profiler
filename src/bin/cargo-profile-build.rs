use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser, Debug)]
#[command(name = "cargo-profile-build")]
#[command(about = "Build a Rust project with line profiling instrumentation")]
struct Args {
    /// Path to Cargo.toml
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Package to build
    #[arg(short, long)]
    package: Option<String>,

    /// Build in release mode
    #[arg(long)]
    release: bool,

    /// Directory for all generated artifacts
    #[arg(long)]
    target_dir: Option<PathBuf>,

    /// Build a specific binary
    #[arg(long)]
    bin: Option<String>,

    /// Build a specific example
    #[arg(long)]
    example: Option<String>,

    /// Additional arguments to pass to cargo
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() -> Result<()> {
    // Skip "profile-build" if invoked as "cargo profile-build"
    let args: Vec<String> = std::env::args().collect();
    let args = if args.len() > 1 && args[1] == "profile-build" {
        Args::parse_from(std::iter::once(args[0].clone()).chain(args.into_iter().skip(2)))
    } else {
        Args::parse()
    };

    // Get path to our rustc wrapper
    let wrapper_path = std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Could not find parent directory"))?
        .join("rustc-wrapper");

    if !wrapper_path.exists() {
        anyhow::bail!(
            "rustc-wrapper not found at {}. Please ensure line-profiler is properly installed.",
            wrapper_path.display()
        );
    }

    println!("Building with line profiler instrumentation...");
    println!("Using wrapper: {}", wrapper_path.display());

    let mut cmd = Command::new("cargo");
    cmd.arg("build");

    if args.release {
        cmd.arg("--release");
    }

    if let Some(ref manifest) = args.manifest_path {
        cmd.arg("--manifest-path").arg(manifest);
    }

    if let Some(ref pkg) = args.package {
        cmd.arg("--package").arg(pkg);
    }

    if let Some(ref target) = args.target_dir {
        cmd.arg("--target-dir").arg(target);
    }

    if let Some(ref bin) = args.bin {
        cmd.arg("--bin").arg(bin);
    }

    if let Some(ref example) = args.example {
        cmd.arg("--example").arg(example);
    }

    cmd.args(&args.args);

    // Set environment variables for instrumentation
    cmd.env("RUSTC_WRAPPER", &wrapper_path);
    cmd.env("LINE_PROFILER_INSTRUMENT", "1");

    // Pass through debug flag value if set
    if let Ok(val) = std::env::var("LINE_PROFILER_DEBUG") {
        cmd.env("LINE_PROFILER_DEBUG", val);
    }

    // Set the runtime library path (in profiler-runtime/target/release)
    let bin_dir = wrapper_path.parent().unwrap();
    let runtime_lib = bin_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("profiler-runtime/target/release/libprofiler_runtime.a");
    if runtime_lib.exists() {
        cmd.env("LINE_PROFILER_RUNTIME_LIB", &runtime_lib);
    }

    // Ensure debug info is present for line information
    let existing_flags = std::env::var("RUSTFLAGS").unwrap_or_default();
    let new_flags = if existing_flags.is_empty() {
        "-C debuginfo=2".to_string()
    } else {
        format!("{} -C debuginfo=2", existing_flags)
    };
    cmd.env("RUSTFLAGS", new_flags);

    let status = cmd.status()?;

    if status.success() {
        println!("\nBuild completed with line profiling instrumentation.");
        println!("Run with 'cargo profile-run' to execute and collect profiling data.");
    } else {
        eprintln!("\nBuild failed.");
    }

    std::process::exit(status.code().unwrap_or(1));
}
