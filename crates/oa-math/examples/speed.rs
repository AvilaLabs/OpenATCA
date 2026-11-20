//! Timing of each function against std over 10 million inputs. Not a test.
//! Run: `cargo run --release -p oa-math --example speed`
//!
//! The std column calls the platform maths library; the oa-math column is this crate. Input
//! generation (xorshift) is timed separately and subtracted. Each figure is the best of three
//! runs, measured in CPU time of this thread's process (Linux `/proc/self/schedstat`, falling
//! back to the wall clock), so a loaded machine does not inflate it much.

use std::hint::black_box;
use std::time::Instant;

const N: u64 = 10_000_000;

struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / 9_007_199_254_740_992.0
    }
}

/// CPU seconds this process has run so far, if the kernel reports it.
fn cpu_seconds() -> Option<f64> {
    // seal-allow: /proc/self/schedstat, this process's own CPU time
    let text = std::fs::read_to_string("/proc/self/schedstat").ok()?;
    let ns: f64 = text.split_whitespace().next()?.parse().ok()?;
    Some(ns * 1e-9)
}

/// Seconds for N calls of `f(rng)`, summing the results so the work is kept; best of three.
fn time(f: &dyn Fn(&mut Rng) -> f64) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..3 {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let wall = Instant::now();
        let cpu = cpu_seconds();
        let mut acc = 0.0;
        for _ in 0..N {
            acc += f(&mut rng);
        }
        black_box(acc);
        let secs = match (cpu, cpu_seconds()) {
            (Some(a), Some(b)) => b - a,
            _ => wall.elapsed().as_secs_f64(),
        };
        if secs < best {
            best = secs;
        }
    }
    best
}

fn row(name: &str, gen: f64, ours: &dyn Fn(&mut Rng) -> f64, std: &dyn Fn(&mut Rng) -> f64) {
    let a = (time(ours) - gen).max(0.0) / N as f64 * 1e9;
    let b = (time(std) - gen).max(0.0) / N as f64 * 1e9;
    println!("{name:28} {a:8.1} ns {b:8.1} ns   x{:.1}", a / b);
}

fn main() {
    let gen = time(&|r| r.unit());
    println!(
        "{:28} {:>11} {:>11}",
        "function (input domain)", "oa-math", "std"
    );
    row(
        "sin [-100, 100]",
        gen,
        &|r| oa_math::sin(black_box(r.unit() * 200.0 - 100.0)),
        &|r| black_box(r.unit() * 200.0 - 100.0).sin(),
    );
    row(
        "sin [1e6, 1e15] (Payne-Hanek)",
        gen,
        &|r| oa_math::sin(black_box(1e6 + r.unit() * 1e15)),
        &|r| black_box(1e6 + r.unit() * 1e15).sin(),
    );
    row(
        "cos [-100, 100]",
        gen,
        &|r| oa_math::cos(black_box(r.unit() * 200.0 - 100.0)),
        &|r| black_box(r.unit() * 200.0 - 100.0).cos(),
    );
    row(
        "sin_cos [-100, 100]",
        gen,
        &|r| {
            let (s, c) = oa_math::sin_cos(black_box(r.unit() * 200.0 - 100.0));
            s + c
        },
        &|r| {
            let (s, c) = black_box(r.unit() * 200.0 - 100.0).sin_cos();
            s + c
        },
    );
    row(
        "tan [-100, 100]",
        gen,
        &|r| oa_math::tan(black_box(r.unit() * 200.0 - 100.0)),
        &|r| black_box(r.unit() * 200.0 - 100.0).tan(),
    );
    row(
        "atan [-10, 10]",
        gen,
        &|r| oa_math::atan(black_box(r.unit() * 20.0 - 10.0)),
        &|r| black_box(r.unit() * 20.0 - 10.0).atan(),
    );
    row(
        "atan2 [-10, 10]^2",
        gen,
        &|r| {
            oa_math::atan2(
                black_box(r.unit() * 20.0 - 10.0),
                black_box(r.unit() * 20.0 - 10.0),
            )
        },
        &|r| black_box(r.unit() * 20.0 - 10.0).atan2(black_box(r.unit() * 20.0 - 10.0)),
    );
    row(
        "asin [-1, 1]",
        gen,
        &|r| oa_math::asin(black_box(r.unit() * 2.0 - 1.0)),
        &|r| black_box(r.unit() * 2.0 - 1.0).asin(),
    );
    row(
        "acos [-1, 1]",
        gen,
        &|r| oa_math::acos(black_box(r.unit() * 2.0 - 1.0)),
        &|r| black_box(r.unit() * 2.0 - 1.0).acos(),
    );
    row(
        "exp [-700, 700]",
        gen,
        &|r| oa_math::exp(black_box(r.unit() * 1400.0 - 700.0)),
        &|r| black_box(r.unit() * 1400.0 - 700.0).exp(),
    );
    row(
        "ln [1e-3, 1e3]",
        gen,
        &|r| oa_math::ln(black_box(r.unit() * 1000.0 + 1e-3)),
        &|r| black_box(r.unit() * 1000.0 + 1e-3).ln(),
    );
    row(
        "pow x [0.1, 100], y [-5, 5]",
        gen,
        &|r| {
            oa_math::pow(
                black_box(r.unit() * 100.0 + 0.1),
                black_box(r.unit() * 10.0 - 5.0),
            )
        },
        &|r| black_box(r.unit() * 100.0 + 0.1).powf(black_box(r.unit() * 10.0 - 5.0)),
    );
    row(
        "powi x [0.5, 2], n = 17",
        gen,
        &|r| oa_math::powi(black_box(r.unit() + 0.5), black_box(17)),
        &|r| black_box(r.unit() + 0.5).powi(black_box(17)),
    );
    row(
        "hypot [-100, 100]^2",
        gen,
        &|r| {
            oa_math::hypot(
                black_box(r.unit() * 200.0 - 100.0),
                black_box(r.unit() * 200.0 - 100.0),
            )
        },
        &|r| black_box(r.unit() * 200.0 - 100.0).hypot(black_box(r.unit() * 200.0 - 100.0)),
    );
    row(
        "cbrt [-1e6, 1e6]",
        gen,
        &|r| oa_math::cbrt(black_box(r.unit() * 2e6 - 1e6)),
        &|r| black_box(r.unit() * 2e6 - 1e6).cbrt(),
    );
}
