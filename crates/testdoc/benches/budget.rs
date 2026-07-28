//! Shared bench plumbing: budget loading, timing, and the pass/fail report.
//!
//! Included via `mod budget;` by each bench target rather than living in the library, so it stays
//! out of the shipped crate's public surface.

use std::time::{Duration, Instant};

/// Ceilings loaded from `benches/budgets.toml` at the workspace root.
pub struct Budgets {
    entries: Vec<(String, f64)>,
    /// How far past a ceiling a measurement may go before it fails.
    tolerance: f64,
}

impl Budgets {
    pub fn load() -> Budgets {
        // The bench binary's CWD is the workspace root under `cargo bench`, but resolve relative to
        // the manifest so it also works when invoked directly.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../benches/budgets.toml")
            .canonicalize()
            .expect("benches/budgets.toml must exist — the gate is meaningless without it");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        Budgets::parse(&text)
    }

    /// Minimal TOML reading: `key = number` lines under `[section]` headers, `#` comments.
    ///
    /// A full TOML parser would be a new dependency for a file this crate writes and reads itself,
    /// against the workspace's minimal-graph rule. The format is deliberately trivial enough that
    /// this is not a parser worth having opinions about.
    fn parse(text: &str) -> Budgets {
        let mut entries = Vec::new();
        let mut tolerance = 2.0;
        let mut section = String::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.trim().to_string();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let Ok(value) = v.parse::<f64>() else {
                continue;
            };
            if section.is_empty() && k == "tolerance_factor" {
                tolerance = value;
            } else {
                entries.push((format!("{section}.{k}"), value));
            }
        }
        Budgets { entries, tolerance }
    }

    /// Record a measurement against its ceiling, collecting rather than panicking so one bench run
    /// reports every violation instead of only the first.
    pub fn check(&self, key: &str, measured: f64, failures: &mut Vec<String>) {
        let Some((_, ceiling)) = self.entries.iter().find(|(k, _)| k == key) else {
            println!("  ! no budget for '{key}' (measured {measured:.3}) — add one");
            failures.push(format!("missing budget for '{key}'"));
            return;
        };
        let limit = ceiling * self.tolerance;
        if measured > limit {
            println!("  FAIL {key}: {measured:.3} > {limit:.3} (budget {ceiling:.3})");
            failures.push(format!(
                "{key}: measured {measured:.3}, ceiling {ceiling:.3} x{:.1} = {limit:.3}",
                self.tolerance
            ));
        } else {
            println!("  ok   {key}: {measured:.3} (budget {ceiling:.3}, limit {limit:.3})");
        }
    }

    /// Record a *deterministic counter* against its ceiling, with no tolerance applied.
    ///
    /// The tolerance factor exists for one reason, stated at the top of `benches/budgets.toml`:
    /// shared runners vary by 10-30% between runs. A work counter does not vary at all — the same
    /// document and the same edit produce the same number on every machine — so doubling its
    /// ceiling buys nothing and can cost everything. Spec 0044 re-baselined
    /// `incremental_pages_reflowed` to 260 on a 499-page document, at which point the doubled limit
    /// of 520 sat above every value the counter can physically take and the gate could no longer
    /// fail. A budget whose limit is unreachable is not a budget, which is the same lesson this
    /// repo already paid for with CI jobs that were not required contexts.
    pub fn check_exact(&self, key: &str, measured: f64, failures: &mut Vec<String>) {
        let Some((_, ceiling)) = self.entries.iter().find(|(k, _)| k == key) else {
            println!("  ! no budget for '{key}' (measured {measured:.3}) — add one");
            failures.push(format!("missing budget for '{key}'"));
            return;
        };
        if measured > *ceiling {
            println!("  FAIL {key}: {measured:.3} > {ceiling:.3} (exact, no tolerance)");
            failures.push(format!(
                "{key}: measured {measured:.3}, ceiling {ceiling:.3} (exact)"
            ));
        } else {
            println!("  ok   {key}: {measured:.3} (budget {ceiling:.3}, exact)");
        }
    }
}

/// Run `f` `n` times and return the fastest run.
///
/// Minimum rather than mean: the thing being measured is how long the work *takes*, and every
/// source of noise on a shared runner (a neighbouring build, a scheduler preemption) only ever adds
/// time. The minimum is the closest estimate of the true cost, and it is far more stable run to run
/// than a mean that a single outlier can drag.
pub fn min_of(n: usize, mut f: impl FnMut()) -> Duration {
    assert!(n > 0);
    let mut best = Duration::MAX;
    for _ in 0..n {
        let start = Instant::now();
        f();
        best = best.min(start.elapsed());
    }
    best
}

/// Print the verdict and exit non-zero if anything blew its budget.
pub fn report(failures: Vec<String>) {
    if failures.is_empty() {
        println!("\nall budgets met.");
        return;
    }
    eprintln!("\n{} budget violation(s):", failures.len());
    for f in &failures {
        eprintln!("  - {f}");
    }
    eprintln!(
        "\nIf a change legitimately makes this slower, update benches/budgets.toml in the same \
         commit and say why in the message."
    );
    std::process::exit(1);
}
