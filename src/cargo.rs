use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// Represents a cargo command that we're wrapping
pub struct CargoCommand {
    pub manifest_path: Option<PathBuf>,
    pub package: Option<String>,
    pub release: bool,
    pub target_dir: Option<PathBuf>,
    pub extra_args: Vec<String>,
}

impl CargoCommand {
    pub fn new() -> Self {
        Self {
            manifest_path: None,
            package: None,
            release: false,
            target_dir: None,
            extra_args: Vec::new(),
        }
    }

    /// Run cargo build with our custom rustc wrapper
    pub fn build(&self, wrapper_path: &std::path::Path) -> Result<std::process::Output> {
        let mut cmd = Command::new("cargo");
        cmd.arg("build");

        if self.release {
            cmd.arg("--release");
        }

        if let Some(ref manifest) = self.manifest_path {
            cmd.arg("--manifest-path").arg(manifest);
        }

        if let Some(ref pkg) = self.package {
            cmd.arg("--package").arg(pkg);
        }

        if let Some(ref target) = self.target_dir {
            cmd.arg("--target-dir").arg(target);
        }

        // Set our wrapper as RUSTC_WRAPPER
        cmd.env("RUSTC_WRAPPER", wrapper_path);

        // Enable LLVM IR emission
        cmd.env("RUSTFLAGS", "-C save-temps --emit=llvm-ir");

        cmd.args(&self.extra_args);

        cmd.output().context("Failed to execute cargo build")
    }

    /// Run the built binary
    pub fn run(&self, binary_path: &std::path::Path, args: &[String]) -> Result<std::process::Output> {
        Command::new(binary_path)
            .args(args)
            .output()
            .context("Failed to run profiled binary")
    }
}

impl Default for CargoCommand {
    fn default() -> Self {
        Self::new()
    }
}
