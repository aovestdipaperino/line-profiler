use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser, Debug)]
#[command(name = "cargo-profile-run")]
#[command(about = "Run a Rust project with line profiling")]
struct Args {
    /// Path to Cargo.toml
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Package to run
    #[arg(short, long)]
    package: Option<String>,

    /// Build in release mode
    #[arg(long)]
    release: bool,

    /// Name of the binary to run
    #[arg(long)]
    bin: Option<String>,

    /// Directory for all generated artifacts
    #[arg(long)]
    target_dir: Option<PathBuf>,

    /// Run a specific example
    #[arg(long)]
    example: Option<String>,

    /// Expose profiling data on this port (disables periodic flush)
    #[arg(long)]
    port: Option<u16>,

    /// Arguments to pass to the binary
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() -> Result<()> {
    // Skip "profile-run" if invoked as "cargo profile-run"
    let args: Vec<String> = std::env::args().collect();
    let args = if args.len() > 1 && args[1] == "profile-run" {
        Args::parse_from(std::iter::once(args[0].clone()).chain(args.into_iter().skip(2)))
    } else {
        Args::parse()
    };

    // Get path to cargo-profile-build (same directory as this binary)
    let self_path = std::env::current_exe()?;
    let bin_dir = self_path.parent().ok_or_else(|| anyhow::anyhow!("No parent dir"))?;
    let profile_build = bin_dir.join("cargo-profile-build");

    // First, build with profiling
    let mut build_cmd = Command::new(&profile_build);

    if let Some(ref manifest) = args.manifest_path {
        build_cmd.arg("--manifest-path").arg(manifest);
    }
    if let Some(ref pkg) = args.package {
        build_cmd.arg("--package").arg(pkg);
    }
    if args.release {
        build_cmd.arg("--release");
    }
    if let Some(ref target) = args.target_dir {
        build_cmd.arg("--target-dir").arg(target);
    }
    if let Some(ref bin) = args.bin {
        build_cmd.arg("--bin").arg(bin);
    }
    if let Some(ref example) = args.example {
        build_cmd.arg("--example").arg(example);
    }

    let build_status = build_cmd.status()?;

    if !build_status.success() {
        std::process::exit(build_status.code().unwrap_or(1));
    }

    // Determine the binary path
    let target_dir = args.target_dir.clone().unwrap_or_else(|| PathBuf::from("target"));
    let profile_dir = if args.release { "release" } else { "debug" };

    // Get binary name - example, explicit bin, or package name
    let (bin_name, is_example) = if let Some(ref example) = args.example {
        (example.clone(), true)
    } else if let Some(ref bin) = args.bin {
        (bin.clone(), false)
    } else {
        (get_package_name(&args.manifest_path)?, false)
    };

    let binary_path = if is_example {
        target_dir.join(profile_dir).join("examples").join(&bin_name)
    } else {
        target_dir.join(profile_dir).join(&bin_name)
    };

    // Find the mapping file
    let mapping_file = if is_example {
        // Examples have mapping files in target/<profile>/examples/
        let examples_dir = target_dir.join(profile_dir).join("examples");
        find_mapping_file(&examples_dir, &bin_name)?
    } else {
        let deps_dir = target_dir.join(profile_dir).join("deps");
        find_mapping_file(&deps_dir, &bin_name)?
    };

    println!("\n=== Running with line profiler ===\n");

    // Run the binary with mapping file path
    let mut run_cmd = Command::new(&binary_path);
    run_cmd.args(&args.args);

    if let Some(ref map_path) = mapping_file {
        run_cmd.env("LINE_PROFILER_MAP", map_path);
    }

    if let Some(port) = args.port {
        run_cmd.env("LINE_PROFILER_PORT", port.to_string());
    }

    let status = run_cmd.status()?;

    std::process::exit(status.code().unwrap_or(1));
}

fn get_package_name(manifest_path: &Option<PathBuf>) -> Result<String> {
    let manifest = manifest_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("Cargo.toml"));

    let content = std::fs::read_to_string(&manifest)?;

    // Simple TOML parsing for package name
    for line in content.lines() {
        if let Some(name) = line.strip_prefix("name = ") {
            let name = name.trim().trim_matches('"');
            return Ok(name.to_string());
        }
    }

    anyhow::bail!("Could not find package name in Cargo.toml")
}

fn find_mapping_file(deps_dir: &PathBuf, bin_name: &str) -> Result<Option<PathBuf>> {
    if !deps_dir.exists() {
        return Ok(None);
    }

    // Rust uses underscores in crate names, so convert dashes
    let crate_name = bin_name.replace('-', "_");

    // Look for <crate_name>-*.profiler-map
    for entry in std::fs::read_dir(deps_dir)? {
        let entry = entry?;
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with(&crate_name) && name.ends_with(".profiler-map") {
                return Ok(Some(path));
            }
        }
    }

    Ok(None)
}
