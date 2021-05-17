use integer::Integer;
use traits::Zero;

use biguint::BigUint;

struct MontyReducer<'a> {
    n: &'a BigUint,
    n0inv: u32,
}

// Calculate the modular inverse of `num`, using Extended GCD.
//
// Reference:
// Brent & Zimmermann, Modern Computer Arithmetic, v0.5.9, Algorithm 1.20
fn inv_mod_u32(num: u32) -> u32 {
    // num needs to be relatively prime to 2**32 -- i.e. it must be odd.
    assert!(num % 2 != 0);

    let mut a: i64 = i64::from(num);
    let mut b: i64 = i64::from(u32::max_value()) + 1;

    // ExtendedGcd
    // Input: positive integers a and b
    // Output: integers (g, u, v) such that g = gcd(a, b) = ua + vb
    // As we don't need v for modular inverse, we don't calculate it.

    // 1: (u, w) <- (1, 0)
    let mut u = 1;
    let mut w = 0;
    // 3: while b != 0
    while b != 0 {
        // 4: (q, r) <- DivRem(a, b)
        let q = a / b;
        let r = a % b;
        // 5: (a, b) <- (b, r)
        a = b;
        b = r;
        // 6: (u, w) <- (w, u - qw)
        let m = u - w * q;
        u = w;
        w = m;
    }

    assert!(a == 1);
    // Downcasting acts like a mod 2^32 too.
    u as u32
}

impl<'a> MontyReducer<'a> {
    fn new(n: &'a BigUint) -> Self {
        let n0inv = inv_mod_u32(n.data[0]);
        MontyReducer { n: n, n0inv: n0inv }
    }
}

// Montgomery Reduction
//
// Reference:
// Brent & Zimmermann, Modern Computer Arithmetic, v0.5.9, Algorithm 2.6
fn monty_redc(a: BigUint, mr: &MontyReducer) -> BigUint {
    let mut c = a.data;
    let n = &mr.n.data;
    let n_size = n.len();

    // Allocate sufficient work space
    c.resize(2 * n_size + 2, 0);

    // β is the size of a word, in this case 32 bits. So "a mod β" is
    // equivalent to masking a to 32 bits.
    // mu <- -N^(-1) mod β
    let mu = 0u32.wrapping_sub(mr.n0inv);

    // 1: for i = 0 to (n-1)
    for i in 0..n_size {
        // 2: q_i <- mu*c_i mod β
        let q_i = c[i].wrapping_mul(mu);

        // 3: C <- C + q_i * N * β^i
        super::algorithms::mac_digit(&mut c[i..], n, q_i);
    }

    // 4: R <- C * β^(-n)
    // This is an n-word bitshift, equivalent to skipping n words.
    let ret = BigUint::new(c[n_size..].to_vec());

    // 5: if R >= β^n then return R-N else return R.
    if ret < *mr.n {
        ret
    } else {
        ret - mr.n
    }
}

// Montgomery Multiplication
fn monty_mult(a: BigUint, b: &BigUint, mr: &MontyReducer) -> BigUint {
    monty_redc(a * b, mr)
}

// Montgomery Squaring
fn monty_sqr(a: BigUint, mr: &MontyReducer) -> BigUint {
    // TODO: Replace with an optimised squaring function
    monty_redc(&a * &a, mr)
}

pub fn monty_modpow(a: &BigUint, exp: &BigUint, modulus: &BigUint) -> BigUint {
    let mr = MontyReducer::new(modulus);

    // Calculate the Montgomery parameter
    let mut v = vec![0; modulus.data.len()];
    v.push(1);
    let r = BigUint::new(v);

    // Map the base to the Montgomery domain
    let mut apri = a * &r % modulus;

    // Binary exponentiation
    let mut ans = &r % modulus;
    let mut e = exp.clone();
    while !e.is_zero() {
        if e.is_odd() {
            ans = monty_mult(ans, &apri, &mr);
        }
        apri = monty_sqr(apri, &mr);
        e >>= 1;
    }

    // Map the result back to the residues domain
    monty_redc(ans, &mr)
}
