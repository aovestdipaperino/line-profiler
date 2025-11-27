fn fibonacci(n: u32) -> u64 {
    if n <= 1 {
        return n as u64;
    }
    fibonacci(n - 1) + fibonacci(n - 2)
}

fn main() {
    let n = 20;
    println!("Computing fibonacci({})", n);
    let result = fibonacci(n);
    println!("Result: {}", result);
}
