//! Where a run spends its time, switched on by `SONICOP_PROFILE=1`.
//!
//! The registry holds hundreds of cops and every one of them is put to every file, so the question
//! that decides what is worth optimizing is never "which cop is slow" but "what does a cop cost on
//! a file it has nothing to say about". Answering that needs a per-cop tally taken from inside the
//! run rather than a sampling profiler, which attributes a shared helper to whoever called it last.
//!
//! Off, this costs one relaxed load per cop invocation.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Room for every cop the registry can hold; the tables are indexed by a cop's position in it.
const SLOTS: usize = 512;

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);

static RULE_NANOS: [AtomicU64; SLOTS] = [ZERO; SLOTS];
static RULE_CALLS: [AtomicU64; SLOTS] = [ZERO; SLOTS];
static PHASE_NANOS: [AtomicU64; Phase::COUNT] = [ZERO; Phase::COUNT];
static PHASE_CALLS: [AtomicU64; Phase::COUNT] = [ZERO; Phase::COUNT];

/// The parts of a run that are not one cop's work.
#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Read,
    Parse,
    Index,
    Syntax,
    Directives,
    Sort,
}

impl Phase {
    const COUNT: usize = 6;
    const NAMES: [&'static str; Self::COUNT] =
        ["read", "parse", "index", "syntax", "directives", "sort"];
}

pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

#[inline]
pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Times `body` against one cop's slot. The closure is called either way, so a caller never has to
/// branch on whether profiling is on.
#[inline]
pub(crate) fn rule<T>(index: usize, body: impl FnOnce() -> T) -> T {
    if !enabled() || index >= SLOTS {
        return body();
    }
    let started = Instant::now();
    let value = body();
    RULE_NANOS[index].fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    RULE_CALLS[index].fetch_add(1, Ordering::Relaxed);
    value
}

#[inline]
pub(crate) fn phase<T>(phase: Phase, body: impl FnOnce() -> T) -> T {
    if !enabled() {
        return body();
    }
    let started = Instant::now();
    let value = body();
    PHASE_NANOS[phase as usize].fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    PHASE_CALLS[phase as usize].fetch_add(1, Ordering::Relaxed);
    value
}

/// Writes the tally to stderr, slowest cop first.
pub(crate) fn report(names: &[&'static str]) {
    if !enabled() {
        return;
    }
    let mut rows: Vec<(&str, u64, u64)> = names
        .iter()
        .enumerate()
        .take(SLOTS)
        .map(|(index, name)| {
            (
                *name,
                RULE_NANOS[index].load(Ordering::Relaxed),
                RULE_CALLS[index].load(Ordering::Relaxed),
            )
        })
        .filter(|(_, nanos, calls)| *nanos > 0 || *calls > 0)
        .collect();
    rows.sort_by_key(|(_, nanos, _)| std::cmp::Reverse(*nanos));
    let total: u64 = rows.iter().map(|(_, nanos, _)| nanos).sum();
    let calls: u64 = rows.iter().map(|(_, _, calls)| calls).sum();

    eprintln!("--- sonicop profile ---");
    for (index, name) in Phase::NAMES.iter().enumerate() {
        let nanos = PHASE_NANOS[index].load(Ordering::Relaxed);
        let count = PHASE_CALLS[index].load(Ordering::Relaxed);
        if count == 0 {
            continue;
        }
        eprintln!(
            "phase {name:<12} {:8.3} s  {count:>8} calls  {:8.1} us/call",
            nanos as f64 / 1e9,
            nanos as f64 / count as f64 / 1e3
        );
    }
    eprintln!(
        "cops total {:.3} s over {calls} invocations ({:.1} us each)",
        total as f64 / 1e9,
        total as f64 / calls.max(1) as f64 / 1e3
    );
    eprintln!(
        "{:<52} {:>9} {:>9} {:>10}",
        "cop", "seconds", "calls", "us/call"
    );
    for (name, nanos, count) in &rows {
        eprintln!(
            "{name:<52} {:>9.3} {count:>9} {:>10.2}",
            *nanos as f64 / 1e9,
            *nanos as f64 / (*count).max(1) as f64 / 1e3
        );
    }
}
