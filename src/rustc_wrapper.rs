use anyhow::{Context, Result};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::instrument::Instrumenter;

/// Collapse multiple spaces into one, but preserve spaces inside string literals (c"..." or "...")
fn collapse_spaces_outside_strings(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut in_string = false;
    let mut prev_was_space = false;
    let chars: Vec<char> = line.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        // Check for string start (c" or just ")
        if !in_string && c == '"' {
            in_string = true;
            prev_was_space = false;
            result.push(c);
            i += 1;
            continue;
        }

        // Check for c"..." style strings
        if !in_string && c == 'c' && i + 1 < chars.len() && chars[i + 1] == '"' {
            in_string = true;
            prev_was_space = false;
            result.push(c);
            result.push('"');
            i += 2;
            continue;
        }

        // Check for string end (handle escaped quotes)
        if in_string && c == '"' {
            // Check if this quote is escaped
            let mut backslash_count = 0;
            let mut j = result.len();
            while j > 0 && result.as_bytes()[j - 1] == b'\\' {
                backslash_count += 1;
                j -= 1;
            }
            // Quote is escaped if preceded by odd number of backslashes
            if backslash_count % 2 == 0 {
                in_string = false;
            }
            result.push(c);
            prev_was_space = false;
            i += 1;
            continue;
        }

        // Inside string - preserve everything
        if in_string {
            result.push(c);
            i += 1;
            continue;
        }

        // Outside string - collapse multiple spaces
        if c == ' ' {
            if !prev_was_space {
                result.push(c);
                prev_was_space = true;
            }
        } else {
            result.push(c);
            prev_was_space = false;
        }
        i += 1;
    }

    result
}

/// A wrapper around rustc that instruments the generated LLVM IR
pub struct RustcWrapper {
    /// Original rustc arguments
    args: Vec<String>,
    /// Path to the actual rustc
    rustc_path: PathBuf,
    /// Whether to enable instrumentation
    instrument: bool,
    /// Debug mode
    debug: bool,
}

impl RustcWrapper {
    pub fn from_env() -> Result<Self> {
        let args: Vec<String> = env::args().skip(1).collect();

        // The first argument when used as RUSTC_WRAPPER is the path to rustc
        let rustc_path = if !args.is_empty() {
            PathBuf::from(&args[0])
        } else {
            PathBuf::from("rustc")
        };

        // Check if instrumentation is enabled via environment variable
        let instrument = env::var("LINE_PROFILER_INSTRUMENT").is_ok();
        let debug = env::var("LINE_PROFILER_DEBUG")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
            .unwrap_or(false);

        Ok(Self {
            args: args.into_iter().skip(1).collect(),
            rustc_path,
            instrument,
            debug,
        })
    }

    pub fn new(rustc_path: PathBuf, args: Vec<String>) -> Self {
        Self {
            args,
            rustc_path,
            instrument: true,
            debug: env::var("LINE_PROFILER_DEBUG")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
                .unwrap_or(false),
        }
    }

    /// Check if this is a primary compilation (not a build script or proc macro)
    fn is_primary_package(&self) -> bool {
        !self.args.iter().any(|arg| {
            arg == "--crate-type=proc-macro"
                || arg.contains("build_script_build")
                || arg.contains("build-script-build")
        })
    }

    /// Check if this compilation is for the target crate (not dependencies)
    fn is_target_crate(&self) -> bool {
        // Check for the primary package marker that cargo sets
        env::var("CARGO_PRIMARY_PACKAGE").is_ok()
    }

    /// Check if this is a binary crate (produces an executable)
    fn is_binary_crate(&self) -> bool {
        // Check for --crate-type=bin or --crate-type bin (space separated)
        for i in 0..self.args.len() {
            if self.args[i] == "--crate-type=bin" {
                return true;
            }
            if self.args[i] == "--crate-type" && i + 1 < self.args.len() && self.args[i + 1] == "bin" {
                return true;
            }
        }
        false
    }

    /// Get the crate name from arguments
    fn get_crate_name(&self) -> Option<String> {
        for i in 0..self.args.len() {
            if self.args[i] == "--crate-name" && i + 1 < self.args.len() {
                return Some(self.args[i + 1].clone());
            }
        }
        None
    }

    /// Get the output path from arguments
    fn get_output_path(&self) -> Option<PathBuf> {
        // Check for -o first
        for i in 0..self.args.len() {
            if self.args[i] == "-o" && i + 1 < self.args.len() {
                return Some(PathBuf::from(&self.args[i + 1]));
            }
        }

        // Check for --out-dir and construct path
        let mut out_dir = None;
        let mut crate_name = None;
        let mut extra_filename = String::new();

        for i in 0..self.args.len() {
            if self.args[i] == "--out-dir" && i + 1 < self.args.len() {
                out_dir = Some(PathBuf::from(&self.args[i + 1]));
            }
            if self.args[i] == "--crate-name" && i + 1 < self.args.len() {
                crate_name = Some(self.args[i + 1].clone());
            }
            if self.args[i].starts_with("-C") && i + 1 < self.args.len() {
                if self.args[i + 1].starts_with("extra-filename=") {
                    extra_filename = self.args[i + 1]
                        .trim_start_matches("extra-filename=")
                        .to_string();
                }
            }
            if self.args[i].starts_with("extra-filename=") {
                extra_filename = self.args[i]
                    .trim_start_matches("extra-filename=")
                    .to_string();
            }
        }

        if let (Some(dir), Some(name)) = (out_dir, crate_name) {
            return Some(dir.join(format!("{}{}", name, extra_filename)));
        }

        None
    }

    /// Run rustc with instrumentation
    pub fn run(&self) -> Result<i32> {
        let should_instrument =
            self.instrument && self.is_primary_package() && self.is_target_crate() && self.is_binary_crate();

        if !should_instrument {
            return self.run_rustc_passthrough();
        }

        if self.debug {
            eprintln!(
                "[line-profiler] Instrumenting crate: {}",
                self.get_crate_name().unwrap_or_else(|| "<unknown>".to_string())
            );
        }

        self.run_with_instrumentation()
    }

    fn run_rustc_passthrough(&self) -> Result<i32> {
        let status = Command::new(&self.rustc_path)
            .args(&self.args)
            .status()
            .context("Failed to execute rustc")?;

        Ok(status.code().unwrap_or(1))
    }

    fn run_with_instrumentation(&self) -> Result<i32> {
        // Create a persistent temp directory for instrumented objects
        let instrument_dir = env::var("LINE_PROFILER_INSTRUMENT_DIR")
            .map(PathBuf::from)
            .ok();

        let temp_dir = if let Some(ref dir) = instrument_dir {
            // Use the shared directory
            std::fs::create_dir_all(dir)?;
            dir.clone()
        } else {
            // Create a new temp directory
            let dir = env::temp_dir().join(format!("line-profiler-{}", std::process::id()));
            std::fs::create_dir_all(&dir)?;
            dir
        };

        let crate_name = self.get_crate_name().unwrap_or_else(|| "unknown".to_string());

        if self.debug {
            eprintln!("[line-profiler] Instrument dir: {}", temp_dir.display());
            eprintln!("[line-profiler] Crate name: {}", crate_name);
        }

        // Step 1: Emit LLVM IR with debug info
        let ir_path = temp_dir.join(format!("{}.ll", crate_name));

        if let Err(e) = self.emit_llvm_ir(&ir_path) {
            eprintln!("[line-profiler] Error emitting IR: {}", e);
            return self.run_rustc_passthrough();
        }

        if !ir_path.exists() {
            if self.debug {
                eprintln!("[line-profiler] Warning: IR file not created at {}, falling back to normal compilation", ir_path.display());
            }
            return self.run_rustc_passthrough();
        }

        // Step 2: Instrument the IR
        let mut instrumenter = Instrumenter::new();
        let instrumented_ir = match instrumenter.instrument_file(&ir_path) {
            Ok(ir) => ir,
            Err(e) => {
                eprintln!("[line-profiler] Error instrumenting IR: {}", e);
                return self.run_rustc_passthrough();
            }
        };

        let instrumented_path = temp_dir.join(format!("{}_instrumented.ll", crate_name));
        std::fs::write(&instrumented_path, &instrumented_ir)?;

        // Write the mapping file for later use
        let mapping_path = self.get_mapping_path();
        if let Err(e) = instrumenter.write_mapping(&mapping_path) {
            eprintln!("[line-profiler] Warning: Failed to write mapping: {}", e);
        } else if self.debug {
            eprintln!("[line-profiler] Wrote mapping to: {}", mapping_path.display());
        }

        // Step 3: Compile instrumented IR to object file
        let obj_path = temp_dir.join(format!("{}.o", crate_name));
        if let Err(e) = self.compile_ir_to_object(&instrumented_path, &obj_path) {
            eprintln!("[line-profiler] Error compiling IR: {}", e);
            return self.run_rustc_passthrough();
        }

        // Step 4: Run rustc with custom linker that uses our instrumented object
        self.run_rustc_with_linker_wrapper(&obj_path, &temp_dir)
    }

    fn emit_llvm_ir(&self, output_path: &Path) -> Result<()> {
        // Build clean args list, removing things we'll override
        let mut new_args = Vec::new();
        let mut skip_next = false;

        for arg in &self.args {
            if skip_next {
                skip_next = false;
                continue;
            }

            // Skip flags we'll override
            if arg == "-o" || arg == "--out-dir" {
                skip_next = true;
                continue;
            }
            if arg.starts_with("--emit") || arg.starts_with("--out-dir=") {
                continue;
            }
            // Skip -C incremental since we're doing a one-shot build
            if arg == "-C" {
                // Check if next arg is incremental
                if let Some(pos) = self.args.iter().position(|a| std::ptr::eq(a, arg)) {
                    if let Some(next) = self.args.get(pos + 1) {
                        if next.starts_with("incremental=") {
                            skip_next = true;
                            continue;
                        }
                    }
                }
            }

            new_args.push(arg.clone());
        }

        // Add emit flag for LLVM IR only
        new_args.push("--emit=llvm-ir".to_string());

        // Set output directory (rustc will create name.ll in this dir)
        let output_dir = output_path.parent().unwrap_or(Path::new("."));
        new_args.push("--out-dir".to_string());
        new_args.push(output_dir.to_string_lossy().to_string());

        if self.debug {
            eprintln!("[line-profiler] Running rustc with args: {:?}", new_args);
        }

        let output = Command::new(&self.rustc_path)
            .args(&new_args)
            .output()
            .context("Failed to emit LLVM IR")?;

        if !output.status.success() {
            eprintln!(
                "[line-profiler] IR emission failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            anyhow::bail!("Failed to emit LLVM IR");
        }

        // Find the generated .ll file
        let crate_name = self.get_crate_name().unwrap_or_else(|| "unknown".to_string());
        let expected_ll = output_dir.join(format!("{}.ll", crate_name));

        if expected_ll.exists() {
            // Rename to our expected path if different
            if expected_ll != output_path {
                std::fs::rename(&expected_ll, output_path)?;
            }
        } else {
            // Look for any .ll file in the output directory
            for entry in std::fs::read_dir(output_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "ll") {
                    if self.debug {
                        eprintln!("[line-profiler] Found IR file: {}", path.display());
                    }
                    std::fs::rename(&path, output_path)?;
                    break;
                }
            }
        }

        Ok(())
    }

    fn compile_ir_to_object(&self, ir_path: &Path, obj_path: &Path) -> Result<()> {
        // Clean the IR to be compatible with older clang/llc versions
        let ir_content = std::fs::read_to_string(ir_path)?;

        let mut cleaned_lines = Vec::new();
        for line in ir_content.lines() {
            // Skip debug record intrinsics (#dbg_declare, #dbg_value, etc.)
            if line.trim().starts_with("#dbg_") {
                continue;
            }

            let mut cleaned_line = line.to_string();

            // Remove 'nuw', 'nsw', and 'nneg' from instructions (newer LLVM features)
            // These appear in getelementptr, trunc, zext, sext, and other instructions
            if cleaned_line.contains(" nuw ") || cleaned_line.contains(" nsw ") || cleaned_line.contains(" nneg ") {
                cleaned_line = cleaned_line.replace(" nuw ", " ");
                cleaned_line = cleaned_line.replace(" nsw ", " ");
                cleaned_line = cleaned_line.replace(" nneg ", " ");
            }

            // Remove 'samesign' prefix from icmp predicates (LLVM 2024 feature)
            // e.g., "icmp samesign ult" becomes "icmp ult"
            if cleaned_line.contains("icmp samesign ") {
                cleaned_line = cleaned_line.replace("icmp samesign ", "icmp ");
            }

            // Remove newer parameter attributes that clang doesn't understand
            while let Some(start) = cleaned_line.find("captures(") {
                if let Some(end) = cleaned_line[start..].find(')') {
                    cleaned_line = format!(
                        "{}{}",
                        &cleaned_line[..start],
                        &cleaned_line[start + end + 1..]
                    );
                } else {
                    break;
                }
            }

            // Remove 'dead_on_unwind' and 'dead_on_return' attributes
            cleaned_line = cleaned_line.replace(" dead_on_unwind", "");
            cleaned_line = cleaned_line.replace("dead_on_unwind ", "");
            cleaned_line = cleaned_line.replace(" dead_on_return", "");
            cleaned_line = cleaned_line.replace("dead_on_return ", "");

            // Remove 'writable' attribute
            cleaned_line = cleaned_line.replace(" writable", "");

            // Remove newer memory(...) attribute format
            while let Some(start) = cleaned_line.find("memory(") {
                if let Some(end) = cleaned_line[start..].find(')') {
                    cleaned_line = format!(
                        "{}{}",
                        &cleaned_line[..start],
                        &cleaned_line[start + end + 1..]
                    );
                } else {
                    break;
                }
            }

            // Remove standalone range(...) attribute
            let mut search_from = 0;
            while let Some(pos) = cleaned_line[search_from..].find("range(") {
                let start = search_from + pos;
                let is_standalone = start == 0
                    || cleaned_line.as_bytes().get(start - 1).map_or(true, |&c| c == b' ' || c == b',');

                if !is_standalone {
                    search_from = start + 6;
                    continue;
                }

                let mut depth = 0;
                let mut end = start;
                for (i, c) in cleaned_line[start..].char_indices() {
                    if c == '(' {
                        depth += 1;
                    } else if c == ')' {
                        depth -= 1;
                        if depth == 0 {
                            end = start + i;
                            break;
                        }
                    }
                }
                if depth == 0 && end > start {
                    cleaned_line = format!(
                        "{}{}",
                        &cleaned_line[..start],
                        &cleaned_line[end + 1..]
                    );
                } else {
                    break;
                }
            }

            // Clean up multiple spaces (but not inside string literals)
            cleaned_line = collapse_spaces_outside_strings(&cleaned_line);
            cleaned_line = cleaned_line.replace(" ,", ",");

            cleaned_lines.push(cleaned_line);
        }

        let cleaned_ir = cleaned_lines.join("\n");
        let cleaned_path = ir_path.with_extension("cleaned.ll");
        std::fs::write(&cleaned_path, cleaned_ir)?;

        // Try llc first (more compatible with LLVM IR)
        let llc_result = Command::new("llc")
            .args([
                "-O2",
                "-filetype=obj",
                "-o",
                &obj_path.to_string_lossy(),
                &cleaned_path.to_string_lossy(),
            ])
            .output();

        match llc_result {
            Ok(output) if output.status.success() => {
                if self.debug {
                    eprintln!("[line-profiler] Compiled IR with llc");
                }
                return Ok(());
            }
            Ok(output) => {
                if self.debug {
                    eprintln!(
                        "[line-profiler] llc failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
            Err(e) => {
                if self.debug {
                    eprintln!("[line-profiler] llc not found: {}", e);
                }
            }
        }

        // Try clang
        let clang_result = Command::new("clang")
            .args([
                "-c",
                "-O2",
                "-o",
                &obj_path.to_string_lossy(),
                &cleaned_path.to_string_lossy(),
            ])
            .output();

        match clang_result {
            Ok(output) if output.status.success() => {
                if self.debug {
                    eprintln!("[line-profiler] Compiled IR with clang");
                }
                return Ok(());
            }
            Ok(output) => {
                if self.debug {
                    eprintln!(
                        "[line-profiler] clang failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
            Err(e) => {
                if self.debug {
                    eprintln!("[line-profiler] clang not found: {}", e);
                }
            }
        }

        anyhow::bail!("Could not compile IR to object file. Please ensure llc or clang is installed.")
    }

    fn run_rustc_with_linker_wrapper(&self, obj_path: &Path, _instrument_dir: &Path) -> Result<i32> {
        // Strategy: Instead of letting rustc compile and link (which produces its own objects),
        // we emit only metadata from rustc, then manually link our instrumented object.

        // Get output path
        let output_path = match self.get_output_path() {
            Some(p) => p,
            None => {
                if self.debug {
                    eprintln!("[line-profiler] Could not determine output path, falling back");
                }
                return self.run_rustc_passthrough();
            }
        };

        // Step 1: Run rustc to emit metadata only (needed for cargo)
        let mut metadata_args = Vec::new();
        let mut skip_next = false;

        for arg in &self.args {
            if skip_next {
                skip_next = false;
                continue;
            }

            // Skip -o since we'll set our own for metadata
            if arg == "-o" {
                skip_next = true;
                continue;
            }

            // Modify --emit to only include metadata
            if arg.starts_with("--emit=") {
                metadata_args.push("--emit=dep-info,metadata".to_string());
                continue;
            }

            metadata_args.push(arg.clone());
        }

        if self.debug {
            eprintln!("[line-profiler] Emitting metadata only");
        }

        let metadata_status = Command::new(&self.rustc_path)
            .args(&metadata_args)
            .status()
            .context("Failed to emit metadata")?;

        if !metadata_status.success() {
            return Ok(metadata_status.code().unwrap_or(1));
        }

        // Step 2: Link manually using our instrumented object + Rust libraries
        let runtime_lib = self.get_runtime_lib_path();

        // Find Rust sysroot for libraries
        let sysroot_output = Command::new(&self.rustc_path)
            .arg("--print=sysroot")
            .output()
            .context("Failed to get sysroot")?;

        let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot_output.stdout).trim());

        // Get target triple
        let target_output = Command::new(&self.rustc_path)
            .args(["-vV"])
            .output()
            .context("Failed to get rustc version")?;

        let target_info = String::from_utf8_lossy(&target_output.stdout);
        let target = target_info
            .lines()
            .find(|l| l.starts_with("host:"))
            .map(|l| l.trim_start_matches("host:").trim())
            .unwrap_or("unknown");

        let lib_dir = sysroot.join("lib").join("rustlib").join(target).join("lib");

        if self.debug {
            eprintln!("[line-profiler] Sysroot: {}", sysroot.display());
            eprintln!("[line-profiler] Lib dir: {}", lib_dir.display());
        }

        // Build linker command
        let mut link_cmd = Command::new("cc");

        // Add our instrumented object
        link_cmd.arg(obj_path);

        // Add Rust standard libraries
        link_cmd.arg(format!("-L{}", lib_dir.display()));

        // Find and add rlib files for std
        for entry in std::fs::read_dir(&lib_dir)? {
            let entry = entry?;
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "rlib" {
                    link_cmd.arg(&path);
                }
            }
        }

        // Find dependency directory and add ALL rlib files from it
        // (includes both direct dependencies and their transitive dependencies)
        for i in 0..self.args.len() {
            if self.args[i] == "-L" && i + 1 < self.args.len() {
                let lib_path = &self.args[i + 1];
                if lib_path.starts_with("dependency=") {
                    let deps_dir = &lib_path[11..];
                    link_cmd.arg(format!("-L{}", deps_dir));

                    // Add all rlib files from deps directory
                    if let Ok(entries) = std::fs::read_dir(deps_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if let Some(ext) = path.extension() {
                                if ext == "rlib" {
                                    if self.debug {
                                        eprintln!("[line-profiler] Adding dep: {}", path.display());
                                    }
                                    link_cmd.arg(&path);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Add profiler runtime
        if let Some(ref lib) = runtime_lib {
            link_cmd.arg(lib);
            if self.debug {
                eprintln!("[line-profiler] Adding runtime: {}", lib.display());
            }
        }

        // Add system libraries
        #[cfg(target_os = "macos")]
        {
            link_cmd.args(["-lSystem", "-lc", "-lm", "-lobjc"]);
            link_cmd.args(["-framework", "Foundation", "-framework", "CoreFoundation"]);
            link_cmd.arg("-arch");
            link_cmd.arg(if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" });
        }

        #[cfg(target_os = "linux")]
        {
            link_cmd.args(["-lpthread", "-ldl", "-lc", "-lm"]);
        }

        // Output
        link_cmd.arg("-o");
        link_cmd.arg(&output_path);

        if self.debug {
            eprintln!("[line-profiler] Linking: {:?}", link_cmd);
        }

        let link_status = link_cmd.status().context("Failed to link")?;

        if link_status.success() {
            if self.debug {
                eprintln!("[line-profiler] Successfully linked: {}", output_path.display());
            }
        }

        Ok(link_status.code().unwrap_or(1))
    }

    fn get_mapping_path(&self) -> PathBuf {
        // Store mapping alongside the output
        if let Some(out) = self.get_output_path() {
            out.with_extension("profiler-map")
        } else {
            PathBuf::from("profiler-map.txt")
        }
    }

    fn get_runtime_lib_path(&self) -> Option<PathBuf> {
        // First check environment variable
        if let Ok(path) = env::var("LINE_PROFILER_RUNTIME_LIB") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Some(path);
            }
        }

        // Look for the runtime library in standard locations
        let exe_dir = env::current_exe().ok()?.parent()?.to_path_buf();

        // Check in same directory as executable
        let lib_path = exe_dir.join("libprofiler_runtime.a");
        if lib_path.exists() {
            return Some(lib_path);
        }

        // Check in ../lib relative to executable
        let lib_path = exe_dir.parent()?.join("lib").join("libprofiler_runtime.a");
        if lib_path.exists() {
            return Some(lib_path);
        }

        None
    }
}
