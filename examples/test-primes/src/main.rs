// ANSI color codes
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

fn main() {
    println!("Testing primality of 10 large random numbers\n");

    let mut rng = SimpleRng::new(12345);

    for i in 1..=10000 {
        let n = rng.next_u64() | 1; // ensure odd
        let is_prime = miller_rabin(n, 20);
        println!(
            "#{}: {} is {}",
            i,
            n,
            if is_prime {
                format!("{}PRIME{}", GREEN, RESET)
            } else {
                format!("{}COMPOSITE{}", RED, RESET)
            }
        );
    }
}

// Simple xorshift64 RNG (no dependencies)
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
}

// Modular exponentiation: (base^exp) mod modulus
fn mod_pow(mut base: u128, mut exp: u128, modulus: u128) -> u128 {
    if modulus == 1 {
        return 0;
    }
    let mut result: u128 = 1;
    base %= modulus;
    while exp > 0 {
        if exp % 2 == 1 {
            result = (result * base) % modulus;
        }
        exp /= 2;
        base = (base * base) % modulus;
    }
    result
}

// Miller-Rabin primality test
fn miller_rabin(n: u64, iterations: u32) -> bool {
    if n < 2 {
        return false;
    }
    if n == 2 || n == 3 {
        return true;
    }
    if n % 2 == 0 {
        return false;
    }

    // Write n-1 as 2^r * d
    let mut d = n - 1;
    let mut r = 0u32;
    while d % 2 == 0 {
        d /= 2;
        r += 1;
    }

    // Witness loop
    let witnesses: [u64; 12] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37];

    for &a in witnesses.iter().take(iterations as usize) {
        if a >= n {
            continue;
        }
        if !miller_rabin_witness(n, d, r, a) {
            return false;
        }
    }

    true
}

fn miller_rabin_witness(n: u64, d: u64, r: u32, a: u64) -> bool {
    let n128 = n as u128;
    let mut x = mod_pow(a as u128, d as u128, n128) as u64;

    if x == 1 || x == n - 1 {
        return true;
    }

    for _ in 0..r - 1 {
        x = mod_pow(x as u128, 2, n128) as u64;
        if x == n - 1 {
            return true;
        }
    }

    false
}
