//! Custom linker wrapper for line profiler instrumentation
//!
//! This wrapper intercepts the link step and:
//! 1. Replaces original object files with instrumented versions
//! 2. Adds the profiler runtime library to the link

use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();

    let debug = env::var("LINE_PROFILER_DEBUG").is_ok();

    if debug {
        eprintln!("[linker-wrapper] Args: {:?}", args);
    }

    // Find the real linker (cc by default)
    let real_linker = env::var("LINE_PROFILER_REAL_LINKER")
        .unwrap_or_else(|_| "cc".to_string());

    // Get the instrumentation directory where we stored .o files
    let instrument_dir = env::var("LINE_PROFILER_INSTRUMENT_DIR")
        .ok()
        .map(PathBuf::from);

    // Get the runtime library path
    let runtime_lib = env::var("LINE_PROFILER_RUNTIME_LIB").ok();

    if debug {
        eprintln!("[linker-wrapper] Real linker: {}", real_linker);
        eprintln!("[linker-wrapper] Instrument dir: {:?}", instrument_dir);
        eprintln!("[linker-wrapper] Runtime lib: {:?}", runtime_lib);
    }

    // Process arguments - look for .o files that have instrumented versions
    let mut new_args: Vec<String> = Vec::new();
    let mut added_runtime = false;

    for arg in &args {
        // Check if this is an object file that we have instrumented
        if let Some(ref dir) = instrument_dir {
            if arg.ends_with(".o") || arg.ends_with(".rlib") {
                let path = Path::new(arg);
                if let Some(filename) = path.file_name() {
                    let instrumented_path = dir.join(filename);
                    if instrumented_path.exists() {
                        if debug {
                            eprintln!("[linker-wrapper] Substituting {} -> {}",
                                arg, instrumented_path.display());
                        }
                        new_args.push(instrumented_path.to_string_lossy().to_string());
                        continue;
                    }
                }
            }
        }

        // Check for -o flag to add runtime before output
        if arg == "-o" && !added_runtime {
            if let Some(ref lib) = runtime_lib {
                if debug {
                    eprintln!("[linker-wrapper] Adding runtime library: {}", lib);
                }

                // Check if this is a static library (.a) or needs specific flags
                // Just link the library normally - our exported symbols will be resolved
                // Don't use -force_load or --whole-archive as they cause duplicate symbols
                new_args.push(lib.clone());
                added_runtime = true;
            }
        }

        new_args.push(arg.clone());
    }

    // If we haven't added the runtime yet, add it at the end (before any output)
    if !added_runtime {
        if let Some(ref lib) = runtime_lib {
            if debug {
                eprintln!("[linker-wrapper] Adding runtime library at end: {}", lib);
            }

            new_args.push(lib.clone());
        }
    }

    if debug {
        eprintln!("[linker-wrapper] Final args: {:?}", new_args);
    }

    // Invoke the real linker
    let status = Command::new(&real_linker)
        .args(&new_args)
        .status()
        .with_context(|| format!("Failed to invoke linker: {}", real_linker))?;

    std::process::exit(status.code().unwrap_or(1));
}

/// Find instrumented object files in a directory
#[allow(dead_code)]
fn find_instrumented_objects(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut objects = Vec::new();

    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "o") {
                objects.push(path);
            }
        }
    }

    Ok(objects)
}
