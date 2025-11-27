use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Global profiler state
static PROFILER: Mutex<Option<Profiler>> = Mutex::new(None);

/// Source location for profiling
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Location {
    pub file: String,
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
    pub children: HashMap<Location, CallNode>,
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
}

/// Active call frame on the stack
#[derive(Debug)]
struct CallFrame {
    location: Location,
    enter_time: Instant,
    child_time: Duration,
}

/// The main profiler structure
#[derive(Debug)]
pub struct Profiler {
    /// Root of the call tree
    root: CallNode,
    /// Current call stack
    stack: Vec<CallFrame>,
    /// Line timing within current function
    current_line_start: Option<(Location, Instant)>,
}

impl Profiler {
    pub fn new() -> Self {
        Self {
            root: CallNode::new(Location {
                file: "<root>".to_string(),
                line: 0,
                column: 0,
            }),
            stack: Vec::new(),
            current_line_start: None,
        }
    }

    /// Enter a function
    pub fn enter_function(&mut self, file: &str, line: u32, column: u32) {
        let location = Location {
            file: file.to_string(),
            line,
            column,
        };

        self.stack.push(CallFrame {
            location,
            enter_time: Instant::now(),
            child_time: Duration::ZERO,
        });
    }

    /// Exit a function
    pub fn exit_function(&mut self) {
        if let Some(frame) = self.stack.pop() {
            let total_time = frame.enter_time.elapsed();
            let self_time = total_time.saturating_sub(frame.child_time);

            // Update the call tree
            let node = self.get_or_create_node(&frame.location);
            node.call_count += 1;
            node.total_time += total_time;
            node.self_time += self_time;

            // Add our time to parent's child_time
            if let Some(parent) = self.stack.last_mut() {
                parent.child_time += total_time;
            }
        }
    }

    /// Record a line hit
    pub fn record_line(&mut self, file: &str, line: u32, column: u32) {
        let now = Instant::now();

        // Finish timing the previous line if any
        if let Some((prev_loc, start)) = self.current_line_start.take() {
            let elapsed = now.duration_since(start);
            let node = self.get_or_create_node(&prev_loc);
            node.self_time += elapsed;
        }

        // Start timing this line
        let location = Location {
            file: file.to_string(),
            line,
            column,
        };
        self.current_line_start = Some((location.clone(), now));

        // Increment call count for this line
        let node = self.get_or_create_node(&location);
        node.call_count += 1;
    }

    fn get_or_create_node(&mut self, location: &Location) -> &mut CallNode {
        // Navigate to the correct position in the tree based on call stack
        let mut current = &mut self.root;

        for frame in &self.stack {
            current = current
                .children
                .entry(frame.location.clone())
                .or_insert_with(|| CallNode::new(frame.location.clone()));
        }

        current
            .children
            .entry(location.clone())
            .or_insert_with(|| CallNode::new(location.clone()))
    }

    /// Get the call tree root
    pub fn root(&self) -> &CallNode {
        &self.root
    }

    /// Print the profile results as a tree
    pub fn print_tree(&self) {
        println!("Line Profile Results:");
        println!("=====================\n");
        self.print_node(&self.root, 0);
    }

    fn print_node(&self, node: &CallNode, depth: usize) {
        let indent = "  ".repeat(depth);

        if node.call_count > 0 {
            println!(
                "{}{}:{} - {} calls, {:.2}ms total, {:.2}ms self",
                indent,
                node.location.file,
                node.location.line,
                node.call_count,
                node.total_time.as_secs_f64() * 1000.0,
                node.self_time.as_secs_f64() * 1000.0,
            );
        }

        // Sort children by total time descending
        let mut children: Vec<_> = node.children.values().collect();
        children.sort_by(|a, b| b.total_time.cmp(&a.total_time));

        for child in children {
            self.print_node(child, depth + 1);
        }
    }
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

// FFI functions called from instrumented code

/// Initialize the profiler
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_init() {
    let mut guard = PROFILER.lock().unwrap();
    *guard = Some(Profiler::new());
}

/// Enter a function (called at function entry)
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_enter(func_id: u64, _location_id: u64) {
    let mut guard = PROFILER.lock().unwrap();
    if let Some(ref mut profiler) = *guard {
        // TODO: Look up actual file/line from func_id
        profiler.enter_function(&format!("func_{}", func_id), 0, 0);
    }
}

/// Exit a function (called at function exit)
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_exit(_func_id: u64) {
    let mut guard = PROFILER.lock().unwrap();
    if let Some(ref mut profiler) = *guard {
        profiler.exit_function();
    }
}

/// Record a line execution
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_line(location_id: u64, line: i32, column: i32) {
    let mut guard = PROFILER.lock().unwrap();
    if let Some(ref mut profiler) = *guard {
        // TODO: Look up actual file from location_id
        profiler.record_line(&format!("loc_{}", location_id), line as u32, column as u32);
    }
}

/// Finalize and print results
#[unsafe(no_mangle)]
pub extern "C" fn __line_profiler_finish() {
    let guard = PROFILER.lock().unwrap();
    if let Some(ref profiler) = *guard {
        profiler.print_tree();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profiler_basic() {
        let mut profiler = Profiler::new();

        profiler.enter_function("test.rs", 10, 1);
        profiler.record_line("test.rs", 11, 5);
        profiler.record_line("test.rs", 12, 5);
        profiler.exit_function();

        assert!(!profiler.root.children.is_empty());
    }
}
