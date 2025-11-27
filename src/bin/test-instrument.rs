use anyhow::Result;
use line_profiler::Instrumenter;
use std::env;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: test-instrument <ir-file>");
        std::process::exit(1);
    }

    let ir_path = PathBuf::from(&args[1]);
    let mut instrumenter = Instrumenter::new();
    let result = instrumenter.instrument_file(&ir_path)?;

    println!("{}", result);

    eprintln!("\n=== Registered Functions ===");
    for (name, id) in instrumenter.functions() {
        eprintln!("  {} -> {}", id, name);
    }

    eprintln!("\n=== Registered Locations ===");
    for (loc, id) in instrumenter.locations() {
        eprintln!("  {} -> {}:{}:{}", id, loc.file, loc.line, loc.column);
    }

    Ok(())
}
