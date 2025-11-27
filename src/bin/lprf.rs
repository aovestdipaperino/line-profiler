//! Profile reader - reads and displays .lprf binary profile files

use anyhow::{Context, Result};
use clap::Parser;
use lprf_parser::{
    generate_subtree, read_profile, read_profile_from_port,
    ProfileHeader, ProfileNode, SymbolMap, TreeNode, TreeNodeType,
};
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

// ============================================================================
// CLI definitions
// ============================================================================

#[derive(Parser, Debug)]
#[command(name = "lprf")]
#[command(about = "Read and display .lprf line profiler results")]
struct Args {
    /// Path to the .lprf profile file
    profile: PathBuf,

    /// Path to the .profiler-map file for symbol resolution
    #[arg(short, long)]
    map: Option<PathBuf>,

    /// Output format
    #[arg(short, long, default_value = "tree")]
    format: OutputFormat,

    /// Sort by (time, calls, self)
    #[arg(short, long, default_value = "time")]
    sort: SortBy,

    /// Only show entries with at least this percentage of total time
    #[arg(long)]
    min_percent: Option<f64>,

    /// Show raw IDs instead of resolved names
    #[arg(long)]
    raw: bool,

    /// Include pre-main runtime startup code (std::rt::lang_start, etc.)
    #[arg(long, default_value = "false")]
    pre_main: bool,

    /// Include library code in output
    #[arg(long, action = clap::ArgAction::Set, value_parser = clap::value_parser!(bool))]
    library: Option<bool>,

    /// Show exclusive (self) time instead of inclusive time in tree view
    #[arg(long)]
    exclusive: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutputFormat {
    /// Tree view (hierarchical)
    Tree,
    /// Flat list sorted by time
    Flat,
    /// CSV output
    Csv,
    /// JSON output
    Json,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum SortBy {
    /// Sort by total time
    Time,
    /// Sort by call count
    Calls,
    /// Sort by self time
    #[value(name = "self")]
    SelfTime,
}

// ============================================================================
// Display helpers
// ============================================================================

/// ANSI color codes
const GREY: &str = "\x1b[90m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

/// Generate a visual percentage bar with color coding
fn format_percent_bar(percent: f64, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);

    // Color based on percentage: green < 25%, yellow 25-75%, red > 75%
    let color = if percent < 25.0 {
        GREEN
    } else if percent < 75.0 {
        YELLOW
    } else {
        RED
    };

    format!(
        "{}{}{}{}{}",
        color,
        "█".repeat(filled),
        GREY,
        "░".repeat(empty),
        RESET
    )
}

fn format_duration(ns: u64) -> String {
    let dur = Duration::from_nanos(ns);
    if dur.as_secs() > 0 {
        format!("{:.3}s", dur.as_secs_f64())
    } else if dur.as_millis() > 0 {
        format!("{:.3}ms", dur.as_secs_f64() * 1000.0)
    } else if dur.as_micros() > 0 {
        format!("{:.3}µs", dur.as_secs_f64() * 1_000_000.0)
    } else {
        format!("{}ns", ns)
    }
}

fn format_timestamp(ns: u64) -> String {
    let dur = Duration::from_nanos(ns);
    let time = UNIX_EPOCH + dur;
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs();
            chrono_format(secs)
        }
        Err(_) => format!("{} ns", ns),
    }
}

fn chrono_format(secs: u64) -> String {
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let mut year = 1970;
    let mut remaining_days = days_since_epoch;

    loop {
        let days_in_year = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_months = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &days in &days_in_months {
        if remaining_days < days {
            break;
        }
        remaining_days -= days;
        month += 1;
    }
    let day = remaining_days + 1;

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        year, month, day, hours, minutes, seconds
    )
}

/// Detect if a mangled function name is from library code
fn is_mangled_library_function(name: &str) -> bool {
    if !name.starts_with("_ZN") {
        return false;
    }

    if name.starts_with("_ZN3std") || name.starts_with("_ZN4core") || name.starts_with("_ZN5alloc") {
        return true;
    }

    if name.starts_with("_ZN2__") {
        return true;
    }

    if name.contains("$LT$core") || name.contains("$LT$std") || name.contains("$LT$alloc") {
        return true;
    }

    if name.contains("4core") || name.contains("3std") || name.contains("5alloc") {
        return true;
    }

    false
}

/// Find the depth of the user's main function
fn find_user_main_depth(nodes: &[ProfileNode], symbol_map: Option<&SymbolMap>, pre_main: bool) -> u16 {
    if pre_main {
        return 0;
    }

    for node in nodes {
        if !node.is_function() {
            continue;
        }

        let name = symbol_map
            .and_then(|m| m.get_function(node.func_id()))
            .unwrap_or("");

        if name.ends_with("::main")
            && !name.starts_with("std::")
            && !name.starts_with("core::")
            && !name.is_empty()
        {
            return node.depth();
        }
    }

    0
}

// ============================================================================
// Display functions
// ============================================================================

fn print_tree(
    header: &ProfileHeader,
    nodes: &[ProfileNode],
    symbol_map: Option<&SymbolMap>,
    args: &Args,
) {
    // Generate the tree using the API
    let include_library = args.library.unwrap_or(false);
    let tree = generate_subtree(header, nodes, symbol_map, include_library, args.pre_main);

    // Print header
    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║                     LINE PROFILER RESULTS                        ║");
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!();
    println!("File: {}", args.profile.display());
    println!("Version: {}", tree.version);
    println!("Timestamp: {}", format_timestamp(tree.timestamp_ns));
    println!("Total time: {}", format_duration(tree.get_total_time()));
    println!();

    // Sort function for children
    let sort_fn: Box<dyn Fn(&TreeNode, &TreeNode) -> std::cmp::Ordering> = match args.sort {
        SortBy::Time => Box::new(|a: &TreeNode, b: &TreeNode| b.inclusive_time_ns.cmp(&a.inclusive_time_ns)),
        SortBy::Calls => Box::new(|a: &TreeNode, b: &TreeNode| b.call_count.cmp(&a.call_count)),
        SortBy::SelfTime => Box::new(|a: &TreeNode, b: &TreeNode| b.exclusive_time_ns.cmp(&a.exclusive_time_ns)),
    };

    // Print tree recursively
    fn print_node(
        node: &TreeNode,
        prefix: &str,
        is_last: bool,
        total_time: u64,
        min_percent: Option<f64>,
        show_exclusive: bool,
        sort_fn: &dyn Fn(&TreeNode, &TreeNode) -> std::cmp::Ordering,
    ) {
        let inclusive_pct = node.inclusive_percent(total_time);

        // Skip if below minimum percentage
        if let Some(min) = min_percent {
            if inclusive_pct < min {
                return;
            }
        }

        // Determine connector
        let connector = if is_last { "└─" } else { "├─" };

        // Choose time to display
        let (display_pct, display_time, time_label) = if show_exclusive {
            (node.exclusive_percent(total_time), node.exclusive_time_ns, "self")
        } else {
            (inclusive_pct, node.inclusive_time_ns, "total")
        };

        // Color coding
        let (color_start, color_end) = if node.is_library { (GREY, RESET) } else { ("", "") };
        let pct_bar = format_percent_bar(display_pct, 10);

        // Print based on node type
        match node.node_type {
            TreeNodeType::Function => {
                println!(
                    "{}{} {}{} {:>5.1}%{} {}{}:{}{}  {} calls, {} total, {} self",
                    prefix,
                    connector,
                    pct_bar,
                    RESET,
                    display_pct,
                    color_start,
                    node.name,
                    YELLOW,
                    node.line,
                    RESET,
                    node.call_count,
                    format_duration(node.inclusive_time_ns),
                    format_duration(node.exclusive_time_ns),
                );
            }
            TreeNodeType::Line => {
                println!(
                    "{}{} {}{} {:>5.1}%{} {}{}{} {} hits, {} {}",
                    prefix,
                    connector,
                    pct_bar,
                    RESET,
                    display_pct,
                    color_start,
                    node.name,
                    RESET,
                    color_end,
                    node.call_count,
                    format_duration(display_time),
                    time_label,
                );
            }
        }

        // Child prefix
        let child_prefix = if is_last {
            format!("{}  ", prefix)
        } else {
            format!("{}│ ", prefix)
        };

        // Sort and print children
        let mut sorted_children: Vec<&TreeNode> = node.children.iter().collect();
        sorted_children.sort_by(|a, b| sort_fn(a, b));

        let num_children = sorted_children.len();
        for (i, child) in sorted_children.iter().enumerate() {
            let child_is_last = i == num_children - 1;
            print_node(child, &child_prefix, child_is_last, total_time, min_percent, show_exclusive, sort_fn);
        }
    }

    // Sort and print roots
    let mut sorted_roots: Vec<&TreeNode> = tree.roots.iter().collect();
    sorted_roots.sort_by(|a, b| sort_fn(a, b));

    let num_roots = sorted_roots.len();
    for (i, root) in sorted_roots.iter().enumerate() {
        let is_last = i == num_roots - 1;
        print_node(root, "", is_last, tree.get_total_time(), args.min_percent, args.exclusive, &*sort_fn);
    }
}

fn print_flat(
    header: &ProfileHeader,
    nodes: &[ProfileNode],
    symbol_map: Option<&SymbolMap>,
    args: &Args,
) {
    let depth_offset = find_user_main_depth(nodes, symbol_map, args.pre_main);

    println!("Total time: {}", format_duration(header.elapsed_ns));
    println!();

    let mut sorted_nodes: Vec<_> = nodes.iter()
        .filter(|n| n.depth() >= depth_offset)
        .collect();
    match args.sort {
        SortBy::Time => sorted_nodes.sort_by(|a, b| b.total_time_ns().cmp(&a.total_time_ns())),
        SortBy::Calls => sorted_nodes.sort_by(|a, b| b.call_count().cmp(&a.call_count())),
        SortBy::SelfTime => sorted_nodes.sort_by(|a, b| b.self_time_ns().cmp(&a.self_time_ns())),
    }

    println!(
        "{:<50} {:>10} {:>12} {:>8} {:>12} {:>8}",
        "Name", "Calls", "Total", "%", "Self", "%"
    );
    println!("{}", "-".repeat(100));

    for node in sorted_nodes {
        let percent = if header.elapsed_ns > 0 {
            (node.total_time_ns() as f64 / header.elapsed_ns as f64) * 100.0
        } else {
            0.0
        };

        let self_percent = if header.elapsed_ns > 0 {
            (node.self_time_ns() as f64 / header.elapsed_ns as f64) * 100.0
        } else {
            0.0
        };

        if let Some(min) = args.min_percent {
            if percent < min {
                continue;
            }
        }

        let (name, is_user) = if node.is_function() {
            let n = if args.raw {
                format!("func_{}", node.func_id())
            } else {
                symbol_map
                    .and_then(|m| m.get_function(node.func_id()))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("func_{}", node.func_id()))
            };
            let is_user_code = (node.line() != 0 || node.column() != 0) && !is_mangled_library_function(&n);
            (format!("{}{}:{}:{}{}", n, YELLOW, node.line(), node.column(), RESET), is_user_code)
        } else {
            let file_info = symbol_map.and_then(|m| m.get_location(node.func_id()));
            if let Some((file, _, _, is_user_code)) = file_info {
                let is_actually_user = *is_user_code && !is_mangled_library_function(file);
                let short_file = file.rsplit('/').next().unwrap_or(file);
                (format!("{}:{}:{}", short_file, node.line(), node.column()), is_actually_user)
            } else {
                let name = symbol_map
                    .and_then(|m| m.get_function(node.func_id()))
                    .unwrap_or("");
                let is_lib = is_mangled_library_function(name);
                (format!("line {}:{}", node.line(), node.column()), !is_lib)
            }
        };

        if !is_user && !args.library.unwrap_or(false) {
            continue;
        }

        let (color_start, color_end) = if is_user { ("", "") } else { (GREY, RESET) };
        let display_name = if name.len() > 50 {
            format!("{}...", &name[..47])
        } else {
            name
        };
        println!(
            "{}{:<50}{} {:>10} {:>12} {:>7.1}% {:>12} {:>7.1}%",
            color_start,
            display_name,
            color_end,
            node.call_count(),
            format_duration(node.total_time_ns()),
            percent,
            format_duration(node.self_time_ns()),
            self_percent,
        );
    }
}

fn print_csv(
    header: &ProfileHeader,
    nodes: &[ProfileNode],
    symbol_map: Option<&SymbolMap>,
    args: &Args,
) {
    let depth_offset = find_user_main_depth(nodes, symbol_map, args.pre_main);

    println!("type,name,func_id,line,column,calls,total_ns,total_pct,self_ns,self_pct,depth");

    for node in nodes.iter().filter(|n| n.depth() >= depth_offset) {
        let percent = if header.elapsed_ns > 0 {
            (node.total_time_ns() as f64 / header.elapsed_ns as f64) * 100.0
        } else {
            0.0
        };

        let self_percent = if header.elapsed_ns > 0 {
            (node.self_time_ns() as f64 / header.elapsed_ns as f64) * 100.0
        } else {
            0.0
        };

        if let Some(min) = args.min_percent {
            if percent < min {
                continue;
            }
        }

        let (node_type, name, is_user) = if node.is_function() {
            let n = if args.raw {
                format!("func_{}", node.func_id())
            } else {
                symbol_map
                    .and_then(|m| m.get_function(node.func_id()))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("func_{}", node.func_id()))
            };
            let is_user_code = node.line() != 0 || node.column() != 0;
            ("function", n, is_user_code)
        } else {
            let file_info = symbol_map.and_then(|m| m.get_location(node.func_id()));
            let (n, is_user_code) = if let Some((file, _, _, is_user)) = file_info {
                let short_file = file.rsplit('/').next().unwrap_or(file);
                (format!("{}:{}:{}", short_file, node.line(), node.column()), *is_user)
            } else {
                let fname = symbol_map
                    .and_then(|m| m.get_function(node.func_id()))
                    .unwrap_or("");
                let is_lib = is_mangled_library_function(fname);
                (format!("{}:{}", node.line(), node.column()), !is_lib)
            };
            ("line", n, is_user_code)
        };

        if !is_user && !args.library.unwrap_or(false) {
            continue;
        }

        println!(
            "{},{},{},{},{},{},{},{:.2},{},{:.2},{}",
            node_type,
            name.replace(',', ";"),
            node.func_id(),
            node.line(),
            node.column(),
            node.call_count(),
            node.total_time_ns(),
            percent,
            node.self_time_ns(),
            self_percent,
            node.depth(),
        );
    }
}

fn print_json(
    header: &ProfileHeader,
    nodes: &[ProfileNode],
    symbol_map: Option<&SymbolMap>,
    args: &Args,
) {
    let depth_offset = find_user_main_depth(nodes, symbol_map, args.pre_main);

    println!("{{");
    println!("  \"version\": {},", header.version);
    println!("  \"timestamp_ns\": {},", header.timestamp);
    println!("  \"elapsed_ns\": {},", header.elapsed_ns);
    println!("  \"nodes\": [");

    let filtered: Vec<_> = nodes
        .iter()
        .filter(|n| n.depth() >= depth_offset)
        .filter(|n| {
            if let Some(min) = args.min_percent {
                let percent = if header.elapsed_ns > 0 {
                    (n.total_time_ns() as f64 / header.elapsed_ns as f64) * 100.0
                } else {
                    0.0
                };
                percent >= min
            } else {
                true
            }
        })
        .filter(|n| {
            let include_lib = args.library.unwrap_or(false);
            if include_lib {
                true
            } else {
                if n.is_function() {
                    n.line() != 0 || n.column() != 0
                } else {
                    let has_user_path = symbol_map
                        .and_then(|m| m.get_location(n.func_id()))
                        .map(|(_, _, _, is_user)| *is_user)
                        .unwrap_or(false);

                    if has_user_path {
                        true
                    } else {
                        let fname = symbol_map
                            .and_then(|m| m.get_function(n.func_id()))
                            .unwrap_or("");
                        !is_mangled_library_function(fname)
                    }
                }
            }
        })
        .collect();

    for (i, node) in filtered.iter().enumerate() {
        let name = if node.is_function() {
            let n = if args.raw {
                format!("func_{}", node.func_id())
            } else {
                symbol_map
                    .and_then(|m| m.get_function(node.func_id()))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("func_{}", node.func_id()))
            };
            format!("{}{}:{}:{}{}", n, YELLOW, node.line(), node.column(), RESET)
        } else {
            let file_info = symbol_map.and_then(|m| m.get_location(node.func_id()));
            if let Some((file, _, _, _)) = file_info {
                let short_file = file.rsplit('/').next().unwrap_or(file);
                format!("{}:{}:{}", short_file, node.line(), node.column())
            } else {
                format!("line {}:{}", node.line(), node.column())
            }
        };

        println!("    {{");
        println!(
            "      \"type\": \"{}\",",
            if node.is_function() {
                "function"
            } else {
                "line"
            }
        );
        println!("      \"name\": \"{}\",", name.replace('"', "\\\""));
        println!("      \"func_id\": {},", node.func_id());
        println!("      \"line\": {},", node.line());
        println!("      \"column\": {},", node.column());
        println!("      \"calls\": {},", node.call_count());
        println!("      \"total_time_ns\": {},", node.total_time_ns());
        println!("      \"self_time_ns\": {},", node.self_time_ns());
        println!("      \"depth\": {},", node.depth());
        println!("      \"parent_func_id\": {}", node.parent_func_id());
        if i < filtered.len() - 1 {
            println!("    }},");
        } else {
            println!("    }}");
        }
    }

    println!("  ]");
    println!("}}");
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Check if profile path is a port (format: ":port")
    let profile_str = args.profile.to_string_lossy();
    let (header, nodes, embedded_map) = if profile_str.starts_with(':') {
        let port: u16 = profile_str[1..].parse()
            .context("Invalid port number")?;
        read_profile_from_port(port)?
    } else {
        read_profile(&args.profile)?
    };

    // Use external map if provided, otherwise use embedded map
    let symbol_map = if let Some(ref map_path) = args.map {
        Some(SymbolMap::from_file(map_path)?)
    } else {
        embedded_map
    };

    // Print based on format
    match args.format {
        OutputFormat::Tree => print_tree(&header, &nodes, symbol_map.as_ref(), &args),
        OutputFormat::Flat => print_flat(&header, &nodes, symbol_map.as_ref(), &args),
        OutputFormat::Csv => print_csv(&header, &nodes, symbol_map.as_ref(), &args),
        OutputFormat::Json => print_json(&header, &nodes, symbol_map.as_ref(), &args),
    }

    Ok(())
}
