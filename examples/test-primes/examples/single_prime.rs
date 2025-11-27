fn mod_pow(mut base: u64, mut exp: u64, modulus: u64) -> u64 {
    let mut result = 1u64;
    base %= modulus;
    while exp > 0 {
        if exp & 1 == 1 {
            result = ((result as u128 * base as u128) % modulus as u128) as u64;
        }
        exp >>= 1;
        base = ((base as u128 * base as u128) % modulus as u128) as u64;
    }
    result
}

fn miller_rabin_witness(n: u64, a: u64, d: u64, r: u32) -> bool {
    let mut x = mod_pow(a, d, n);
    if x == 1 || x == n - 1 {
        return false;
    }
    for _ in 0..r - 1 {
        x = ((x as u128 * x as u128) % n as u128) as u64;
        if x == n - 1 {
            return false;
        }
    }
    true
}

fn miller_rabin(n: u64, iterations: u32) -> bool {
    if n < 2 { return false; }
    if n == 2 || n == 3 { return true; }
    if n % 2 == 0 { return false; }

    let mut d = n - 1;
    let mut r = 0u32;
    while d % 2 == 0 {
        d /= 2;
        r += 1;
    }

    let witnesses: [u64; 7] = [2, 3, 5, 7, 11, 13, 17];
    for &a in witnesses.iter().take(iterations as usize) {
        if a >= n { continue; }
        if miller_rabin_witness(n, a, d, r) {
            return false;
        }
    }
    true
}

fn main() {
    let test_number: u64 = 1_000_000_007;
    println!("Testing if {} is prime...", test_number);

    let is_prime = miller_rabin(test_number, 7);
    println!("{} is {}", test_number, if is_prime { "PRIME" } else { "composite" });
}
