//! Profiler runtime library
//!
//! This library provides the runtime functions called by instrumented code.
//! It is compiled as a static library and linked into instrumented binaries.
//!
//! ## File Format (.lprf)
//!
//! The binary file format for profiling data:
//!
//! ```text
//! Header (28 bytes):
//!   Magic: "LPRF" (4 bytes)
//!   Version: u32 (4 bytes) - currently 1
//!   Timestamp: u64 (8 bytes) - nanoseconds since Unix epoch
//!   Elapsed: u64 (8 bytes) - nanoseconds since profiling started
//!   Node count: u32 (4 bytes)
//!
//! Node entries (variable, 49 bytes each):
//!   Type: u8 (1 = function, 2 = line)
//!   Func ID: u64
//!   Line: u32
//!   Column: u32
//!   Call count: u64
//!   Self time (ns): u64
//!   Total time (ns): u64
//!   Depth: u16
//!   Parent func ID: u64 (u64::MAX for root children)
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufRead, BufWriter, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// File format magic bytes
const MAGIC: &[u8; 4] = b"LPRF";
/// File format version
const VERSION: u32 = 1;
/// Flush interval in seconds
const FLUSH_INTERVAL_SECS: u64 = 5;

/// Global profiler state
static PROFILER: Mutex<Option<Profiler>> = Mutex::new(None);
/// Flag to stop the flush thread
static STOP_FLUSH_THREAD: AtomicBool = AtomicBool::new(false);
/// Flag to track if flush thread is started
static FLUSH_THREAD_STARTED: AtomicBool = AtomicBool::new(false);
/// Flag to stop the server thread
static STOP_SERVER_THREAD: AtomicBool = AtomicBool::new(false);
/// Flag to track if server thread is started
static SERVER_THREAD_STARTED: AtomicBool = AtomicBool::new(false);
/// Debug mode flag (set via LINE_PROFILER_DEBUG env var)
static DEBUG: AtomicBool = AtomicBool::new(false);

/// Check if debug mode is enabled
#[inline]
fn is_debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// Symbol mapping from IDs to names
#[derive(Debug, Default)]
pub struct SymbolMap {
    /// Function ID -> demangled name
    pub functions: HashMap<u64, String>,
    /// Location ID -> (file, line, column, is_user_code)
    pub locations: HashMap<u64, (String, u32, u32, bool)>,
}

impl SymbolMap {
    /// Load symbol map from a .profiler-map file
    pub fn load(path: &str) -> Option<Self> {
        let file = File::open(path).ok()?;
        let reader = BufReader::new(file);
        let mut map = SymbolMap::default();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };

            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            if parts.len() < 3 {
                continue;
            }

            match parts[0] {
                "F" => {
                    // Function mapping: F <id> <mangled_name>
                    if let Ok(id) = parts[1].parse::<u64>() {
                        let name = demangle_name(parts[2]);
                        map.functions.insert(id, name);
                    }
                }
                "L" => {
                    // Location mapping: L <id> <file>:<line>:<col>:<is_user>
                    if let Ok(id) = parts[1].parse::<u64>() {
                        if let Some((file, line, col, is_user)) = parse_location(parts[2]) {
                            map.locations.insert(id, (file, line, col, is_user));
                        }
                    }
                }
                _ => {}
            }
        }

        Some(map)
    }

    /// Get function name by ID
    pub fn get_function(&self, id: u64) -> Option<&str> {
        self.functions.get(&id).map(|s| s.as_str())
    }

    /// Get location info by ID: (file, line, column, is_user_code)
    pub fn get_location(&self, id: u64) -> Option<&(String, u32, u32, bool)> {
        self.locations.get(&id)
    }
}

/// Demangle a Rust symbol name
fn demangle_name(mangled: &str) -> String {
    // Simple demangling: extract the function name from Rust mangled names
    // Format: _ZN<len>crate<len>module<len>func<hash>E
    if !mangled.starts_with("_ZN") {
        return mangled.to_string();
    }

    let mut result = Vec::new();
    let chars: Vec<char> = mangled.chars().collect();
    let mut i = 3; // Skip "_ZN"

    while i < chars.len() {
        // Read length
        let mut len_str = String::new();
        while i < chars.len() && chars[i].is_ascii_digit() {
            len_str.push(chars[i]);
            i += 1;
        }

        if len_str.is_empty() {
            break;
        }

        let len: usize = match len_str.parse() {
            Ok(l) => l,
            Err(_) => break,
        };

        // Read name segment
        if i + len <= chars.len() {
            let segment: String = chars[i..i + len].iter().collect();
            // Skip hash segments (start with 'h' followed by hex)
            if !segment.starts_with('h') || segment.len() != 17 {
                result.push(segment);
            }
            i += len;
        } else {
            break;
        }
    }

    if result.is_empty() {
        mangled.to_string()
    } else {
        result.join("::")
    }
}

/// Parse a location string like "file:line:col:is_user" (is_user: 0 or 1)
fn parse_location(s: &str) -> Option<(String, u32, u32, bool)> {
    let parts: Vec<&str> = s.rsplitn(4, ':').collect();
    if parts.len() >= 4 {
        // New format with is_user_code flag
        let is_user = parts[0] == "1";
        let col = parts[1].parse().ok()?;
        let line = parts[2].parse().ok()?;
        let file = parts[3].to_string();
        Some((file, line, col, is_user))
    } else if parts.len() >= 3 {
        // Old format without is_user_code flag (assume user code)
        let col = parts[0].parse().ok()?;
        let line = parts[1].parse().ok()?;
        let file = parts[2].to_string();
        Some((file, line, col, true))
    } else {
        None
    }
}

/// Source location for profiling
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Location {
    pub func_id: u64,
    pub line: u32,
    pub column: u32,
}

/// A node in the call tree
#[derive(Debug)]
pub struct CallNode {
    pub location: Location,
    pub self_time: Duration,
    pub total_time: Duration,
    pub call_count: u64,
    pub children: HashMap<Location, Box<CallNode>>,
}

impl CallNode {
    pub fn new(location: Location) -> Self {
        Self {
            location,
            self_time: Duration::ZERO,
            total_time: Duration::ZERO,
            call_count: 0,
            children: HashMap::new(),
        }
    }

    fn add_time(&mut self, self_time: Duration, total_time: Duration) {
        self.self_time += self_time;
        self.total_time += total_time;
        self.call_count += 1;
    }
}

/// Active call frame on the stack
#[derive(Debug)]
struct CallFrame {
    func_id: u64,
    enter_time: Instant,
    child_time: Duration,
    last_line_time: Option<Instant>,
    last_line_loc: Option<Location>,
}

/// The main profiler structure
pub struct Profiler {
    /// Root of the call tree (synthetic root node)
    root: CallNode,
    /// Current call stack
    stack: Vec<CallFrame>,
    /// Start time for the profiling session
    start_time: Instant,
    /// Output file path
    output_path: String,
    /// Symbol map for name resolution
    symbol_map: Option<SymbolMap>,
    /// Port to serve data on (if set, disables periodic flush)
    port: Option<u16>,
}

impl Profiler {
    pub fn new() -> Self {
        // Get output path from environment or use default
        let output_path = std::env::var("LINE_PROFILER_OUTPUT")
            .unwrap_or_else(|_| "profile.lprf".to_string());

        // Load symbol map if available
        let symbol_map = std::env::var("LINE_PROFILER_MAP")
            .ok()
            .and_then(|path| SymbolMap::load(&path));

        if is_debug() {
            if let Some(ref map) = symbol_map {
                eprintln!("[profiler] Loaded symbol map: {} functions, {} locations",
                    map.functions.len(), map.locations.len());
            }
        }

        // Check for port mode
        let port = std::env::var("LINE_PROFILER_PORT")
            .ok()
            .and_then(|p| p.parse().ok());

        if is_debug() {
            if let Some(p) = port {
                eprintln!("[profiler] Port mode enabled on port {}", p);
            }
        }

        Self {
            root: CallNode::new(Location {
                func_id: u64::MAX,
                line: 0,
                column: 0,
            }),
            stack: Vec::new(),
            start_time: Instant::now(),
            output_path,
            symbol_map,
            port,
        }
    }

    /// Enter a function
    pub fn enter_function(&mut self, func_id: u64, _loc_id: u64) {
        self.stack.push(CallFrame {
            func_id,
            enter_time: Instant::now(),
            child_time: Duration::ZERO,
            last_line_time: None,
            last_line_loc: None,
        });
    }

    /// Exit a function
    pub fn exit_function(&mut self, func_id: u64) {
        // Finish any pending line timing
        let pending_line = if let Some(frame) = self.stack.last_mut() {
            if let (Some(start), Some(loc)) = (frame.last_line_time.take(), frame.last_line_loc.take()) {
                Some((loc, start.elapsed()))
            } else {
                None
            }
        } else {
            None
        };

        if let Some((loc, elapsed)) = pending_line {
            let node = self.get_or_create_node(&loc);
            node.self_time += elapsed;
            node.total_time += elapsed;
        }

        if let Some(frame) = self.stack.pop() {
            if frame.func_id != func_id && is_debug() {
                // Mismatched function exit - stack corruption
                eprintln!(
                    "[profiler] Warning: mismatched function exit (expected {}, got {})",
                    frame.func_id, func_id
                );
            }

            let total_time = frame.enter_time.elapsed();
            let self_time = total_time.saturating_sub(frame.child_time);

            // Record function timing
            let loc = Location {
                func_id: frame.func_id,
                line: 0,
                column: 0,
            };

            // Get the node path from root following call stack
            let node = self.get_or_create_node(&loc);
            node.add_time(self_time, total_time);

            // Add our time to parent's child_time
            if let Some(parent) = self.stack.last_mut() {
                parent.child_time += total_time;
                // Reset parent's line timing to NOW so the next LINE event
                // doesn't include the nested function's duration (which is
                // already recorded in the function node)
                if parent.last_line_time.is_some() {
                    parent.last_line_time = Some(Instant::now());
                }
            }
        }
    }

    /// Record a line execution
    pub fn record_line(&mut self, loc_id: u64, line: i32, column: i32) {
        let now = Instant::now();

        // Finish timing the previous line if any
        let prev_line_data = if let Some(frame) = self.stack.last_mut() {
            if let (Some(start), Some(loc)) = (frame.last_line_time.take(), frame.last_line_loc.take()) {
                Some((loc, now.duration_since(start)))
            } else {
                None
            }
        } else {
            None
        };

        // Record previous line timing
        if let Some((loc, elapsed)) = prev_line_data {
            let node = self.get_or_create_node(&loc);
            node.self_time += elapsed;
            node.total_time += elapsed;
        }

        // Start timing this line
        let location = Location {
            func_id: loc_id,
            line: line as u32,
            column: column as u32,
        };

        // Update frame and get node for new line
        if let Some(frame) = self.stack.last_mut() {
            frame.last_line_time = Some(now);
            frame.last_line_loc = Some(location.clone());
        }

        // Increment call count for this line
        let node = self.get_or_create_node(&location);
        node.call_count += 1;
    }

    fn get_or_create_node(&mut self, location: &Location) -> &mut CallNode {
        // Navigate to the correct position in the tree based on call stack
        let mut current = &mut self.root;

        // First navigate through the call stack
        for frame in &self.stack {
            let frame_loc = Location {
                func_id: frame.func_id,
                line: 0,
                column: 0,
            };
            current = current
                .children
                .entry(frame_loc.clone())
                .or_insert_with(|| Box::new(CallNode::new(frame_loc)));
        }

        // Then get or create the target node
        current
            .children
            .entry(location.clone())
            .or_insert_with(|| Box::new(CallNode::new(location.clone())))
    }

    /// Get total elapsed time
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Count total nodes in the tree
    fn count_nodes(&self) -> u32 {
        fn count_recursive(node: &CallNode) -> u32 {
            let mut count = node.children.len() as u32;
            for child in node.children.values() {
                count += count_recursive(child);
            }
            count
        }
        count_recursive(&self.root)
    }

    /// Flush profiling data to binary file
    ///
    /// Format:
    ///   Header (32 bytes):
    ///     Magic: "LPRF" (4 bytes)
    ///     Version: u32 (4 bytes)
    ///     Timestamp: u64 (8 bytes)
    ///     Elapsed: u64 (8 bytes)
    ///     Node count: u32 (4 bytes)
    ///     Reserved: u32 (4 bytes)
    ///   Symbol map:
    ///     Function count: u32
    ///     Location count: u32
    ///     Functions: [id: u64, name_len: u16, name: bytes]*
    ///     Locations: [id: u64, file_len: u16, file: bytes, line: u32, col: u32]*
    ///   Nodes (49 bytes each)
    pub fn flush_to_binary(&self) -> std::io::Result<()> {
        let file = File::create(&self.output_path)?;
        let mut writer = BufWriter::new(file);

        // Write header
        writer.write_all(MAGIC)?;
        writer.write_all(&VERSION.to_le_bytes())?;

        // Timestamp (nanoseconds since Unix epoch)
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64;
        writer.write_all(&timestamp.to_le_bytes())?;

        // Elapsed time
        let elapsed = self.elapsed().as_nanos() as u64;
        writer.write_all(&elapsed.to_le_bytes())?;

        // Node count
        let node_count = self.count_nodes();
        writer.write_all(&node_count.to_le_bytes())?;

        // Reserved padding (4 bytes)
        writer.write_all(&0u32.to_le_bytes())?;

        // Write symbol map (embedded, before nodes)
        self.write_symbol_map_section(&mut writer)?;

        // Write nodes
        self.write_nodes_binary(&mut writer, &self.root, 0, u64::MAX)?;

        writer.flush()?;

        if is_debug() {
            eprintln!("[profiler] Flushed to {} ({} nodes)", self.output_path, node_count);
        }

        Ok(())
    }

    /// Write symbol map section to binary file
    fn write_symbol_map_section<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let (func_count, loc_count) = if let Some(ref map) = self.symbol_map {
            (map.functions.len() as u32, map.locations.len() as u32)
        } else {
            (0u32, 0u32)
        };

        // Counts
        writer.write_all(&func_count.to_le_bytes())?;
        writer.write_all(&loc_count.to_le_bytes())?;

        if let Some(ref map) = self.symbol_map {
            // Functions
            for (&id, name) in &map.functions {
                writer.write_all(&id.to_le_bytes())?;
                let name_bytes = name.as_bytes();
                writer.write_all(&(name_bytes.len() as u16).to_le_bytes())?;
                writer.write_all(name_bytes)?;
            }

            // Locations (now includes is_user_code flag)
            for (&id, (file, line, col, is_user)) in &map.locations {
                writer.write_all(&id.to_le_bytes())?;
                let file_bytes = file.as_bytes();
                writer.write_all(&(file_bytes.len() as u16).to_le_bytes())?;
                writer.write_all(file_bytes)?;
                writer.write_all(&line.to_le_bytes())?;
                writer.write_all(&col.to_le_bytes())?;
                // Write is_user_code as u8 (0 or 1)
                writer.write_all(&[if *is_user { 1u8 } else { 0u8 }])?;
            }
        }

        Ok(())
    }

    /// Write profile data to a TCP stream
    pub fn write_to_stream(&self, stream: &std::net::TcpStream) -> std::io::Result<()> {
        let mut writer = BufWriter::new(stream);

        // Write header
        writer.write_all(MAGIC)?;
        writer.write_all(&VERSION.to_le_bytes())?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64;
        writer.write_all(&timestamp.to_le_bytes())?;

        let elapsed = self.elapsed().as_nanos() as u64;
        writer.write_all(&elapsed.to_le_bytes())?;

        let node_count = self.count_nodes();
        writer.write_all(&node_count.to_le_bytes())?;

        // Reserved padding
        writer.write_all(&0u32.to_le_bytes())?;

        // Write symbol map
        self.write_symbol_map_section(&mut writer)?;

        // Write nodes
        self.write_nodes_binary(&mut writer, &self.root, 0, u64::MAX)?;

        writer.flush()?;

        Ok(())
    }

    fn write_nodes_binary<W: Write>(
        &self,
        writer: &mut W,
        node: &CallNode,
        depth: u16,
        parent_func_id: u64,
    ) -> std::io::Result<()> {
        for (loc, child) in &node.children {
            // Type: 1 = function (line == 0), 2 = line
            let node_type: u8 = if loc.line == 0 { 1 } else { 2 };
            writer.write_all(&[node_type])?;

            // Func ID
            writer.write_all(&loc.func_id.to_le_bytes())?;

            // Line
            writer.write_all(&loc.line.to_le_bytes())?;

            // Column
            writer.write_all(&loc.column.to_le_bytes())?;

            // Call count
            writer.write_all(&child.call_count.to_le_bytes())?;

            // Self time (ns)
            writer.write_all(&(child.self_time.as_nanos() as u64).to_le_bytes())?;

            // Total time (ns)
            writer.write_all(&(child.total_time.as_nanos() as u64).to_le_bytes())?;

            // Depth
            writer.write_all(&depth.to_le_bytes())?;

            // Parent func ID
            writer.write_all(&parent_func_id.to_le_bytes())?;

            // Recurse into children
            self.write_nodes_binary(writer, child, depth + 1, loc.func_id)?;
        }
        Ok(())
    }

    /// Print the profile results as a tree
    pub fn print_results(&self) {
        println!("\n╔══════════════════════════════════════════════════════════════════╗");
        println!("║                     LINE PROFILER RESULTS                        ║");
        println!("╚══════════════════════════════════════════════════════════════════╝\n");
        println!("Total time: {:.3}ms\n", self.elapsed().as_secs_f64() * 1000.0);

        self.print_node(&self.root, 0, self.elapsed());
    }

    fn print_node(&self, node: &CallNode, depth: usize, total_session_time: Duration) {
        let indent = "  ".repeat(depth);

        // Sort children by total time descending
        let mut children: Vec<_> = node.children.values().collect();
        children.sort_by(|a, b| b.total_time.cmp(&a.total_time));

        for child in children {
            let percent = if total_session_time.as_nanos() > 0 {
                (child.total_time.as_nanos() as f64 / total_session_time.as_nanos() as f64) * 100.0
            } else {
                0.0
            };

            let self_percent = if total_session_time.as_nanos() > 0 {
                (child.self_time.as_nanos() as f64 / total_session_time.as_nanos() as f64) * 100.0
            } else {
                0.0
            };

            if child.location.line == 0 {
                // Function entry - resolve name from symbol map
                let func_name = self.symbol_map
                    .as_ref()
                    .and_then(|m| m.get_function(child.location.func_id))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("func_{}", child.location.func_id));

                println!(
                    "{}├─ {} ({} calls, {:.2}ms total [{:.1}%], {:.2}ms self [{:.1}%])",
                    indent,
                    func_name,
                    child.call_count,
                    child.total_time.as_secs_f64() * 1000.0,
                    percent,
                    child.self_time.as_secs_f64() * 1000.0,
                    self_percent,
                );
            } else {
                // Line - try to get file path from symbol map
                let file_info = self.symbol_map
                    .as_ref()
                    .and_then(|m| m.get_location(child.location.func_id));

                if let Some((file, _, _, _)) = file_info {
                    // Show file:line:column
                    let short_file = file.rsplit('/').next().unwrap_or(file);
                    println!(
                        "{}│  {}:{}:{} ({} hits, {:.3}ms [{:.1}%])",
                        indent,
                        short_file,
                        child.location.line,
                        child.location.column,
                        child.call_count,
                        child.self_time.as_secs_f64() * 1000.0,
                        self_percent,
                    );
                } else {
                    println!(
                        "{}│  line {}:{} ({} hits, {:.3}ms [{:.1}%])",
                        indent,
                        child.location.line,
                        child.location.column,
                        child.call_count,
                        child.self_time.as_secs_f64() * 1000.0,
                        self_percent,
                    );
                }
            }

            self.print_node(child, depth + 1, total_session_time);
        }
    }

    /// Write results to a human-readable text file
    pub fn write_to_file(&self, path: &str) -> std::io::Result<()> {
        let mut file = File::create(path)?;

        writeln!(file, "# Line Profiler Results")?;
        writeln!(file, "# Total time: {:.6}s", self.elapsed().as_secs_f64())?;
        writeln!(file)?;
        writeln!(file, "# Format: type,id,line,column,calls,self_time_ns,total_time_ns,depth,parent_path")?;

        self.write_node(&mut file, &self.root, 0, "")?;

        Ok(())
    }

    fn write_node(&self, file: &mut File, node: &CallNode, depth: usize, parent_path: &str) -> std::io::Result<()> {
        for (loc, child) in &node.children {
            let node_type = if loc.line == 0 { "F" } else { "L" };
            let path = if parent_path.is_empty() {
                format!("{}", loc.func_id)
            } else {
                format!("{}/{}", parent_path, loc.func_id)
            };

            writeln!(
                file,
                "{},{},{},{},{},{},{},{},{}",
                node_type,
                loc.func_id,
                loc.line,
                loc.column,
                child.call_count,
                child.self_time.as_nanos(),
                child.total_time.as_nanos(),
                depth,
                path
            )?;

            self.write_node(file, child, depth + 1, &path)?;
        }
        Ok(())
    }
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

// FFI functions called from instrumented code

/// Start the background flush thread
fn start_flush_thread() {
    // Only start once
    if FLUSH_THREAD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::spawn(|| {
        if is_debug() {
            eprintln!("[profiler] Background flush thread started (interval: {}s)", FLUSH_INTERVAL_SECS);
        }

        loop {
            std::thread::sleep(Duration::from_secs(FLUSH_INTERVAL_SECS));

            if STOP_FLUSH_THREAD.load(Ordering::SeqCst) {
                break;
            }

            // Flush to file
            let mut guard = match PROFILER.lock() {
                Ok(g) => g,
                Err(_) => continue,
            };

            if let Some(ref mut profiler) = *guard {
                if let Err(e) = profiler.flush_to_binary() {
                    if is_debug() {
                        eprintln!("[profiler] Failed to flush: {}", e);
                    }
                }
            }
        }

        if is_debug() {
            eprintln!("[profiler] Background flush thread stopped");
        }
    });
}

/// Start the TCP server thread to serve live profiling data
fn start_server_thread(port: u16) {
    // Only start once
    if SERVER_THREAD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::spawn(move || {
        let listener = match TcpListener::bind(format!("127.0.0.1:{}", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[profiler] Failed to bind to port {}: {}", port, e);
                return;
            }
        };

        if is_debug() {
            eprintln!("[profiler] Server listening on port {}...", port);
        }

        // Set a timeout so we can check the stop flag periodically
        listener.set_nonblocking(true).ok();

        loop {
            if STOP_SERVER_THREAD.load(Ordering::SeqCst) {
                break;
            }

            match listener.accept() {
                Ok((stream, addr)) => {
                    if is_debug() {
                        eprintln!("[profiler] Client connected from {}", addr);
                    }

                    // Get current profiling data snapshot
                    let guard = match PROFILER.lock() {
                        Ok(g) => g,
                        Err(_) => {
                            if is_debug() {
                                eprintln!("[profiler] Failed to lock profiler");
                            }
                            continue;
                        }
                    };

                    if let Some(ref profiler) = *guard {
                        if let Err(e) = profiler.write_to_stream(&stream) {
                            if is_debug() {
                                eprintln!("[profiler] Failed to send data: {}", e);
                            }
                        } else if is_debug() {
                            eprintln!("[profiler] Sent snapshot to client");
                        }
                    }
                    // Lock released here
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No connection available, sleep briefly and retry
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    if is_debug() {
                        eprintln!("[profiler] Accept error: {}", e);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }

        if is_debug() {
            eprintln!("[profiler] Server thread stopped");
        }
    });
}

/// Initialize the profiler (called automatically on first use)
fn ensure_initialized() {
    // Initialize debug flag from environment (only once)
    static DEBUG_INIT: AtomicBool = AtomicBool::new(false);
    if !DEBUG_INIT.swap(true, Ordering::SeqCst) {
        if let Ok(val) = std::env::var("LINE_PROFILER_DEBUG") {
            // Enable debug only if value is "1", "true", or "yes"
            if val == "1" || val.eq_ignore_ascii_case("true") || val.eq_ignore_ascii_case("yes") {
                DEBUG.store(true, Ordering::SeqCst);
            }
        }
    }

    let mut guard = PROFILER.lock().unwrap();
    if guard.is_none() {
        let profiler = Profiler::new();
        let port = profiler.port;
        *guard = Some(profiler);

        #[cfg(not(test))]
        {
            if let Some(p) = port {
                // Port mode: start server thread
                start_server_thread(p);
            } else {
                // Normal mode: start background flush thread
                start_flush_thread();
            }
        }

        // Register atexit handler to print results on program exit
        #[cfg(not(test))]
        auto_init::register_atexit();
    }
}

/// Enter a function (called at function entry)
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_enter(func_id: u64, loc_id: u64) {
    ensure_initialized();
    let mut guard = PROFILER.lock().unwrap();
    if let Some(ref mut profiler) = *guard {
        profiler.enter_function(func_id, loc_id);
    }
}

/// Exit a function (called at function exit)
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_exit(func_id: u64) {
    let mut guard = PROFILER.lock().unwrap();
    if let Some(ref mut profiler) = *guard {
        profiler.exit_function(func_id);
    }
}

/// Record a line execution
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_line(loc_id: u64, line: i32, column: i32) {
    ensure_initialized();
    let mut guard = PROFILER.lock().unwrap();
    if let Some(ref mut profiler) = *guard {
        profiler.record_line(loc_id, line, column);
    }
}

/// Initialize profiler (explicit, optional)
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_init() {
    ensure_initialized();
}

/// Finalize profiling and write results to binary file or serve on port
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_finish() {
    // Stop background threads
    STOP_FLUSH_THREAD.store(true, Ordering::SeqCst);
    STOP_SERVER_THREAD.store(true, Ordering::SeqCst);

    let mut guard = PROFILER.lock().unwrap();
    if let Some(ref mut profiler) = *guard {
        if profiler.port.is_none() {
            // Normal mode: flush to binary file
            if let Err(e) = profiler.flush_to_binary() {
                eprintln!("[profiler] Failed to write final binary file: {}", e);
            } else if is_debug() {
                eprintln!("[profiler] Results written to: {}", profiler.output_path);
            }
        }
        // Port mode: server thread handles connections until stopped
    }
}

// Use ctor crate pattern for automatic cleanup
// The user code should call __line_profiler_finish() or we use atexit

#[cfg(not(test))]
mod auto_init {
    use super::__line_profiler_finish;

    extern "C" fn on_exit() {
        __line_profiler_finish();
    }

    /// Register atexit handler when profiler is first used
    pub fn register_atexit() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            unsafe {
                libc_atexit(on_exit);
            }
        });
    }

    #[cfg(target_os = "macos")]
    unsafe fn libc_atexit(f: extern "C" fn()) {
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        atexit(f);
    }

    #[cfg(target_os = "linux")]
    unsafe fn libc_atexit(f: extern "C" fn()) {
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        atexit(f);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn libc_atexit(_f: extern "C" fn()) {
        // No-op on unsupported platforms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profiler_basic() {
        let mut profiler = Profiler::new();

        profiler.enter_function(1, 0);
        profiler.record_line(1, 10, 5);
        profiler.record_line(1, 11, 5);
        profiler.exit_function(1);

        assert!(profiler.root.children.len() > 0);
    }

    #[test]
    fn test_nested_calls() {
        let mut profiler = Profiler::new();

        profiler.enter_function(1, 0);
        profiler.record_line(1, 10, 5);

        profiler.enter_function(2, 0);
        profiler.record_line(2, 20, 5);
        profiler.exit_function(2);

        profiler.record_line(1, 11, 5);
        profiler.exit_function(1);

        // Check that we have the expected structure
        assert!(profiler.root.children.len() > 0);
    }

    #[test]
    fn test_binary_format() {
        let mut profiler = Profiler::new();

        profiler.enter_function(1, 0);
        profiler.record_line(1, 10, 5);
        profiler.exit_function(1);

        // Create a temp file
        let temp_dir = std::env::temp_dir();
        profiler.output_path = temp_dir.join("test_profile.lprf").to_string_lossy().to_string();

        // Flush and verify file exists
        profiler.flush_to_binary().unwrap();

        assert!(std::path::Path::new(&profiler.output_path).exists());

        // Read and verify header
        let data = std::fs::read(&profiler.output_path).unwrap();
        assert_eq!(&data[0..4], b"LPRF");

        // Clean up
        std::fs::remove_file(&profiler.output_path).ok();
    }
}
