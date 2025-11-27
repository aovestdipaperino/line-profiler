use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Instruments LLVM IR with line profiling code
pub struct Instrumenter {
    /// Maps source locations to unique IDs
    location_ids: HashMap<SourceLocation, u64>,
    /// Maps function names to IDs
    function_ids: HashMap<String, u64>,
    next_loc_id: u64,
    next_fn_id: u64,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub is_user_code: bool,
}

/// Check if a file path belongs to user code (not a library)
fn is_user_code_path(path: &str) -> bool {
    // Check for mangled library function names (used as fallback when file path is unavailable)
    // Standard library: _ZN3std, _ZN4core, _ZN5alloc
    if path.starts_with("_ZN3std") || path.starts_with("_ZN4core") || path.starts_with("_ZN5alloc") {
        return false;
    }
    // Compiler generated: _ZN2__
    if path.starts_with("_ZN2__") {
        return false;
    }
    // Generic types containing core/std/alloc: _ZN79_$LT$core..result... or _ZN...$LT$std...
    if path.contains("$LT$core") || path.contains("$LT$std") || path.contains("$LT$alloc") {
        return false;
    }
    // Also check for core/std/alloc segments in the mangled name
    if path.starts_with("_ZN") && (path.contains("4core") || path.contains("3std") || path.contains("5alloc")) {
        return false;
    }
    // Cargo/package manager paths
    if path.contains("/.cargo/") {
        return false;
    }
    // External crates from crates.io registry
    if path.contains("/.cargo/registry/") {
        return false;
    }
    // External crates from git
    if path.contains("/.cargo/git/") {
        return false;
    }
    if path.contains("/.rustup/") {
        return false;
    }
    // Rust toolchain paths
    if path.contains("/rustc/") {
        return false;
    }
    if path.starts_with("/rustc/") {
        return false;
    }
    // Built dependency code
    if path.contains("/target/debug/deps/") || path.contains("/target/release/deps/") {
        return false;
    }
    // Common system library paths
    if path.contains("/usr/lib/") || path.contains("/usr/local/lib/") {
        return false;
    }
    true
}

impl Instrumenter {
    pub fn new() -> Self {
        Self {
            location_ids: HashMap::new(),
            function_ids: HashMap::new(),
            next_loc_id: 0,
            next_fn_id: 0,
        }
    }

    /// Instrument an LLVM IR file and return the instrumented content
    pub fn instrument_file(&mut self, ir_path: &Path) -> Result<String> {
        let content = fs::read_to_string(ir_path)
            .with_context(|| format!("Failed to read IR file: {}", ir_path.display()))?;

        self.instrument_ir(&content)
    }

    /// Write the location mapping to a file for later lookup
    pub fn write_mapping(&self, path: &Path) -> Result<()> {
        let mut content = String::new();

        content.push_str("# Function mappings\n");
        for (name, id) in &self.function_ids {
            content.push_str(&format!("F {} {}\n", id, name));
        }

        content.push_str("\n# Location mappings\n");
        // Format: L <id> <file>:<line>:<col>:<is_user> (is_user: 1 or 0)
        for (loc, id) in &self.location_ids {
            let is_user = if loc.is_user_code { 1 } else { 0 };
            content.push_str(&format!("L {} {}:{}:{}:{}\n", id, loc.file, loc.line, loc.column, is_user));
        }

        fs::write(path, content)?;
        Ok(())
    }

    /// Instrument LLVM IR content
    pub fn instrument_ir(&mut self, ir: &str) -> Result<String> {
        let mut output = String::new();
        let mut in_function = false;
        let mut function_name = String::new();
        let mut current_fn_id = 0u64;
        let mut after_entry_label = false;
        let mut inserted_enter = false;

        // Regex patterns
        let fn_def_re = Regex::new(r#"^define\s+[^@]*@"?([^"(]+)"?\("#)?;
        let fn_decl_re = Regex::new(r"^declare\s+")?;
        let dbg_loc_re = Regex::new(r"!dbg\s+!(\d+)")?;
        let di_loc_re = Regex::new(r"!(\d+)\s*=\s*!DILocation\(line:\s*(\d+),\s*column:\s*(\d+),\s*scope:\s*!(\d+)")?;
        let di_file_re = Regex::new(r#"!(\d+)\s*=\s*!DIFile\(filename:\s*"([^"]+)"(?:,\s*directory:\s*"([^"]*)")?"#)?;
        let di_subprog_re = Regex::new(r"!(\d+)\s*=\s*(?:distinct\s+)?!DISubprogram\([^)]*file:\s*!(\d+)")?;
        let di_lexblock_re = Regex::new(r"!(\d+)\s*=\s*(?:distinct\s+)?!DILexicalBlock(?:File)?\([^)]*(?:scope:\s*!(\d+))?[^)]*file:\s*!(\d+)")?;
        let ret_re = Regex::new(r"^\s+ret\s+")?;
        let entry_label_re = Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_]*:\s*$")?;

        // First pass: collect metadata
        // DILocation: id -> (line, col, scope_id)
        let mut di_locations: HashMap<u64, (u32, u32, u64)> = HashMap::new();
        // DIFile: id -> (filename, directory)
        let mut di_files: HashMap<u64, (String, String)> = HashMap::new();
        // DISubprogram/DILexicalBlock: id -> file_id
        let mut scope_files: HashMap<u64, u64> = HashMap::new();

        for line in ir.lines() {
            if let Some(caps) = di_loc_re.captures(line) {
                let id: u64 = caps.get(1).unwrap().as_str().parse()?;
                let line_num: u32 = caps.get(2).unwrap().as_str().parse()?;
                let col: u32 = caps.get(3).unwrap().as_str().parse()?;
                let scope: u64 = caps.get(4).unwrap().as_str().parse()?;
                di_locations.insert(id, (line_num, col, scope));
            }

            if let Some(caps) = di_file_re.captures(line) {
                let id: u64 = caps.get(1).unwrap().as_str().parse()?;
                let filename = caps.get(2).unwrap().as_str().to_string();
                let directory = caps.get(3).map(|m| m.as_str().to_string()).unwrap_or_default();
                di_files.insert(id, (filename, directory));
            }

            if let Some(caps) = di_subprog_re.captures(line) {
                let subprog_id: u64 = caps.get(1).unwrap().as_str().parse()?;
                let file_id: u64 = caps.get(2).unwrap().as_str().parse()?;
                scope_files.insert(subprog_id, file_id);
            }

            if let Some(caps) = di_lexblock_re.captures(line) {
                let block_id: u64 = caps.get(1).unwrap().as_str().parse()?;
                let file_id: u64 = caps.get(3).unwrap().as_str().parse()?;
                scope_files.insert(block_id, file_id);
            }
        }

        // Helper to resolve file path from scope
        let resolve_file = |scope_id: u64| -> String {
            if let Some(&file_id) = scope_files.get(&scope_id) {
                if let Some((filename, directory)) = di_files.get(&file_id) {
                    if directory.is_empty() || filename.starts_with('/') {
                        return filename.clone();
                    }
                    return format!("{}/{}", directory, filename);
                }
            }
            String::new()
        };

        // Add profiler runtime declarations at the top, after any existing declarations
        let mut declarations = String::new();
        declarations.push_str("\n; Line profiler instrumentation\n");
        declarations.push_str("declare void @__line_profiler_enter(i64, i64) nounwind\n");
        declarations.push_str("declare void @__line_profiler_exit(i64) nounwind\n");
        declarations.push_str("declare void @__line_profiler_line(i64, i32, i32) nounwind\n");

        let mut added_declarations = false;

        // Second pass: instrument the code
        let mut in_switch = false; // Track multi-line switch statements
        let mut at_block_start = false; // Track if we're at start of a basic block (phi region)
        for line in ir.lines() {
            // Skip if this is a declare line (don't instrument declarations)
            if fn_decl_re.is_match(line) {
                output.push_str(line);
                output.push('\n');

                // Add our declarations after existing declarations
                if !added_declarations {
                    output.push_str(&declarations);
                    added_declarations = true;
                }
                continue;
            }

            // Check for function definition
            if let Some(caps) = fn_def_re.captures(line) {
                in_function = true;
                function_name = caps.get(1).unwrap().as_str().to_string();
                current_fn_id = self.get_or_create_fn_id(&function_name);
                after_entry_label = false;
                inserted_enter = false;

                // Add declarations before first function if not added yet
                if !added_declarations {
                    output.push_str(&declarations);
                    added_declarations = true;
                }

                output.push_str(line);
                output.push('\n');
                continue;
            }

            // Check for entry label in function
            if in_function && !inserted_enter && entry_label_re.is_match(line) {
                output.push_str(line);
                output.push('\n');
                after_entry_label = true;
                continue;
            }

            // Insert enter call after first instruction in function
            if in_function && !inserted_enter && after_entry_label && line.starts_with("  ") {
                // Skip phi nodes and alloca - insert after them
                if !line.contains(" = phi ") && !line.contains(" = alloca ") {
                    output.push_str(&format!(
                        "  call void @__line_profiler_enter(i64 {}, i64 0)\n",
                        current_fn_id
                    ));
                    inserted_enter = true;
                }
            }

            // Insert enter after first non-label line if we haven't yet
            if in_function && !inserted_enter && !after_entry_label && line.starts_with("  ") {
                if !line.contains(" = phi ") && !line.contains(" = alloca ") {
                    output.push_str(&format!(
                        "  call void @__line_profiler_enter(i64 {}, i64 0)\n",
                        current_fn_id
                    ));
                    inserted_enter = true;
                }
            }

            // Check for return - insert exit before it
            if in_function && ret_re.is_match(line) {
                output.push_str(&format!(
                    "  call void @__line_profiler_exit(i64 {})\n",
                    current_fn_id
                ));
            }

            // Check for function end
            if line.trim() == "}" && in_function {
                in_function = false;
                function_name.clear();
                after_entry_label = false;
            }

            // Track multi-line switch statements
            let trimmed = line.trim();
            if trimmed.starts_with("switch ") && trimmed.contains('[') && !trimmed.contains(']') {
                in_switch = true;
            }
            if in_switch && trimmed.contains(']') {
                in_switch = false;
            }

            // Track basic block starts (phi nodes must stay at top)
            if entry_label_re.is_match(line) || (trimmed.ends_with(':') && !trimmed.contains('=')) {
                at_block_start = true;
            }
            // Once we see a non-phi instruction, we're past the phi region
            if at_block_start && trimmed.starts_with('%') && !trimmed.contains(" = phi ") {
                at_block_start = false;
            }
            if at_block_start && !trimmed.contains(" = phi ") && !trimmed.ends_with(':') && !trimmed.starts_with(';') && !trimmed.is_empty() {
                at_block_start = false;
            }

            // If we're in a function and this line has debug info, instrument it
            // BUT skip certain patterns that would break the IR:
            // - invoke instructions (they have 'to' and 'unwind' clauses)
            // - invoke continuation lines (lines with 'to label')
            // - landingpad instructions
            // - resume instructions
            // - unreachable instructions
            // - terminator instructions
            // - inside multi-line switch blocks
            if in_function && inserted_enter {
                let should_skip = in_switch
                    || at_block_start                // don't insert before phi nodes settle
                    || trimmed.contains(" = phi ")   // phi nodes
                    || trimmed.starts_with("invoke ")
                    || trimmed.contains(" invoke ")
                    || trimmed.starts_with("to label")  // invoke continuation
                    || trimmed.contains("to label")     // invoke continuation
                    || trimmed.starts_with("landingpad ")
                    || trimmed.starts_with("resume ")
                    || trimmed.starts_with("unreachable")
                    || trimmed.starts_with("br ")
                    || trimmed.starts_with("switch ")
                    || trimmed.starts_with("indirectbr ")
                    || trimmed.starts_with("catchswitch ")
                    || trimmed.starts_with("catchret ")
                    || trimmed.starts_with("cleanupret ")
                    || trimmed.starts_with("callbr ")
                    || trimmed.starts_with("i8 ")    // switch case labels
                    || trimmed.starts_with("i16 ")   // switch case labels
                    || trimmed.starts_with("i32 ")   // switch case labels
                    || trimmed.starts_with("i64 ")   // switch case labels
                    || trimmed.starts_with(']');     // switch closing bracket

                if !should_skip {
                    if let Some(caps) = dbg_loc_re.captures(line) {
                        let dbg_id: u64 = caps.get(1).unwrap().as_str().parse()?;
                        if let Some(&(line_num, col, scope_id)) = di_locations.get(&dbg_id) {
                            // Don't instrument calls to our profiler
                            if !line.contains("@__line_profiler") {
                                // Resolve file path from scope
                                let file_path = resolve_file(scope_id);
                                let resolved_file = if file_path.is_empty() { function_name.clone() } else { file_path };
                                let loc = SourceLocation {
                                    file: resolved_file.clone(),
                                    line: line_num,
                                    column: col,
                                    is_user_code: is_user_code_path(&resolved_file),
                                };
                                let loc_id = self.get_or_create_loc_id(loc);
                                output.push_str(&format!(
                                    "  call void @__line_profiler_line(i64 {}, i32 {}, i32 {})\n",
                                    loc_id, line_num, col
                                ));
                            }
                        }
                    }
                }
            }

            output.push_str(line);
            output.push('\n');
        }

        // Add declarations at end if we never found a good place
        if !added_declarations {
            output.push_str(&declarations);
        }

        Ok(output)
    }

    fn get_or_create_fn_id(&mut self, name: &str) -> u64 {
        if let Some(&id) = self.function_ids.get(name) {
            id
        } else {
            let id = self.next_fn_id;
            self.next_fn_id += 1;
            self.function_ids.insert(name.to_string(), id);
            id
        }
    }

    fn get_or_create_loc_id(&mut self, loc: SourceLocation) -> u64 {
        if let Some(&id) = self.location_ids.get(&loc) {
            id
        } else {
            let id = self.next_loc_id;
            self.next_loc_id += 1;
            self.location_ids.insert(loc, id);
            id
        }
    }

    /// Get all registered source locations
    pub fn locations(&self) -> &HashMap<SourceLocation, u64> {
        &self.location_ids
    }

    /// Get all registered functions
    pub fn functions(&self) -> &HashMap<String, u64> {
        &self.function_ids
    }
}

impl Default for Instrumenter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instrument_simple_ir() {
        let ir = r#"
define i32 @main() {
entry:
  %0 = alloca i32, !dbg !1
  ret i32 0, !dbg !2
}

!1 = !DILocation(line: 1, column: 1, scope: !3)
!2 = !DILocation(line: 2, column: 5, scope: !3)
!3 = distinct !DISubprogram(name: "main")
"#;

        let mut instrumenter = Instrumenter::new();
        let result = instrumenter.instrument_ir(ir).unwrap();

        assert!(result.contains("@__line_profiler_line"));
        assert!(result.contains("declare void @__line_profiler_enter"));
    }
}
