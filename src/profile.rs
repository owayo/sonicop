//! Where a run spends its time, switched on by `SONICOP_PROFILE=1`.
//!
//! The registry holds hundreds of cops and every one of them is put to every file, so the question
//! that decides what is worth optimizing is never "which cop is slow" but "what does a cop cost on
//! a file it has nothing to say about". Answering that needs a per-cop tally taken from inside the
//! run rather than a sampling profiler, which attributes a shared helper to whoever called it last.
//!
//! Off, this costs one relaxed load per cop invocation.
//!
//! **The tally is per thread.** An earlier version added into one shared table of atomics, and the
//! cost of that dwarfed what it was measuring: neighbouring cops share a cache line (eight `u64`s
//! to sixty-four bytes), so eight workers running eight different cops still fought over the same
//! lines. It reported 44.4 seconds of cop time on a run whose whole wall clock was 3.7 seconds,
//! and an optimisation that halved the reported figure moved the wall clock not at all. A counter
//! only its own thread writes needs no read-modify-write at all -- a relaxed load, an add and a
//! relaxed store -- and the lines stay in the core that owns them.
//!
//! The table is also sized from the registry rather than to a round number. It was 512 slots
//! against 609 cops, so ninety-seven of them were silently dropped: `index >= SLOTS` returned
//! early, and the report neither showed them nor said they were missing.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// How many cops the registry holds, which is how wide each thread's table is.
static SLOTS: AtomicUsize = AtomicUsize::new(0);

/// One thread's tally. Only the thread that owns it writes to it, so the atomics are here to make
/// the read at report time defined rather than to synchronise anything.
struct Counters {
    rule_nanos: Box<[AtomicU64]>,
    rule_calls: Box<[AtomicU64]>,
    phase_nanos: Box<[AtomicU64]>,
    phase_calls: Box<[AtomicU64]>,
    /// Invocations whose cop index was past the end of the table. Should stay zero; if it does
    /// not, the report says so rather than quietly leaving the work out.
    dropped: AtomicU64,
}

impl Counters {
    fn new(slots: usize) -> Self {
        let zeros = |count: usize| {
            (0..count)
                .map(|_| AtomicU64::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice()
        };
        Self {
            rule_nanos: zeros(slots),
            rule_calls: zeros(slots),
            phase_nanos: zeros(Phase::COUNT),
            phase_calls: zeros(Phase::COUNT),
            dropped: AtomicU64::new(0),
        }
    }
}

/// Every thread's table, so the report can add them up. Taken once per thread, not per call.
static REGISTRY: Mutex<Vec<Arc<Counters>>> = Mutex::new(Vec::new());

thread_local! {
    static LOCAL: Arc<Counters> = {
        let counters = Arc::new(Counters::new(SLOTS.load(Ordering::Relaxed)));
        if let Ok(mut registry) = REGISTRY.lock() {
            registry.push(Arc::clone(&counters));
        }
        counters
    };
}

/// Adds to a counter only this thread writes. A plain load-add-store, not a locked
/// read-modify-write -- see the note at the top of the file.
#[inline]
fn bump(counter: &AtomicU64, amount: u64) {
    counter.store(counter.load(Ordering::Relaxed) + amount, Ordering::Relaxed);
}

/// The parts of a run that are not one cop's work.
#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Read,
    Parse,
    Index,
    Syntax,
    Directives,
    Sort,
    /// Walking the tree: what the `ignore` crate spends between entries, pruning included.
    Walk,
    /// `directory_excluded` on every directory the walk offers.
    DirFilter,
    /// Resolving the configuration that governs a discovered path.
    ConfigLookup,
    /// `path_included` / `path_excluded` / `path_hidden` on every file the walk offers.
    PathMatch,
    /// Reading a shebang from an extension-less file.
    Shebang,
    /// Hashing a file's text and configuration into a result-cache key.
    CacheKey,
    /// Reading a cached report back and turning it into offenses.
    CacheLoad,
    /// `VariableForce`, built on the first cop of a file that asks for it.
    Variables,
    /// The code the grammar swallowed, recovered on the first cop that asks.
    Fragments,
    /// The Metrics cops' local-variable replay, built on the first cop that asks.
    MetricLocals,
    /// The lexer token stream, rebuilt on the first cop that asks.
    LayoutTokens,
}

impl Phase {
    const COUNT: usize = 17;
    const NAMES: [&'static str; Self::COUNT] = [
        "read",
        "parse",
        "index",
        "syntax",
        "directives",
        "sort",
        "walk",
        "dir_filter",
        "config_lookup",
        "path_match",
        "shebang",
        "cache_key",
        "cache_load",
        "variables",
        "fragments",
        "metric_locals",
        "layout_tokens",
    ];
}

/// Switches profiling on and sizes the tables to the registry.
pub(crate) fn set_enabled(on: bool, slots: usize) {
    SLOTS.store(slots, Ordering::Relaxed);
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
    if !enabled() {
        return body();
    }
    let started = Instant::now();
    let value = body();
    let elapsed = started.elapsed().as_nanos() as u64;
    LOCAL.with(|counters| match counters.rule_nanos.get(index) {
        Some(nanos) => {
            bump(nanos, elapsed);
            bump(&counters.rule_calls[index], 1);
        }
        None => bump(&counters.dropped, 1),
    });
    value
}

#[inline]
pub(crate) fn phase<T>(phase: Phase, body: impl FnOnce() -> T) -> T {
    if !enabled() {
        return body();
    }
    let started = Instant::now();
    let value = body();
    let elapsed = started.elapsed().as_nanos() as u64;
    LOCAL.with(|counters| {
        bump(&counters.phase_nanos[phase as usize], elapsed);
        bump(&counters.phase_calls[phase as usize], 1);
    });
    value
}

/// The tallies of every thread, added together.
fn totals(slots: usize) -> (Vec<u64>, Vec<u64>, Vec<u64>, Vec<u64>, u64) {
    let mut rule_nanos = vec![0u64; slots];
    let mut rule_calls = vec![0u64; slots];
    let mut phase_nanos = vec![0u64; Phase::COUNT];
    let mut phase_calls = vec![0u64; Phase::COUNT];
    let mut dropped = 0;
    let Ok(registry) = REGISTRY.lock() else {
        return (rule_nanos, rule_calls, phase_nanos, phase_calls, dropped);
    };
    for counters in registry.iter() {
        for (index, total) in rule_nanos.iter_mut().enumerate() {
            if let Some(value) = counters.rule_nanos.get(index) {
                *total += value.load(Ordering::Relaxed);
            }
        }
        for (index, total) in rule_calls.iter_mut().enumerate() {
            if let Some(value) = counters.rule_calls.get(index) {
                *total += value.load(Ordering::Relaxed);
            }
        }
        for (index, total) in phase_nanos.iter_mut().enumerate() {
            *total += counters.phase_nanos[index].load(Ordering::Relaxed);
        }
        for (index, total) in phase_calls.iter_mut().enumerate() {
            *total += counters.phase_calls[index].load(Ordering::Relaxed);
        }
        dropped += counters.dropped.load(Ordering::Relaxed);
    }
    (rule_nanos, rule_calls, phase_nanos, phase_calls, dropped)
}

/// Writes the tally to stderr, slowest cop first.
pub(crate) fn report(names: &[&'static str]) {
    if !enabled() {
        return;
    }
    let slots = SLOTS.load(Ordering::Relaxed).max(names.len());
    let (rule_nanos, rule_calls, phase_nanos, phase_calls, dropped) = totals(slots);

    let mut rows: Vec<(&str, u64, u64)> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                *name,
                rule_nanos.get(index).copied().unwrap_or(0),
                rule_calls.get(index).copied().unwrap_or(0),
            )
        })
        .filter(|(_, nanos, calls)| *nanos > 0 || *calls > 0)
        .collect();
    rows.sort_by_key(|(_, nanos, _)| std::cmp::Reverse(*nanos));
    let total: u64 = rows.iter().map(|(_, nanos, _)| nanos).sum();
    let calls: u64 = rows.iter().map(|(_, _, calls)| calls).sum();
    let threads = REGISTRY.lock().map_or(0, |registry| registry.len());

    eprintln!("--- sonicop profile ---");
    eprintln!(
        "{} cops in the registry, {} of them reached, tallied across {threads} thread(s)",
        names.len(),
        rows.len()
    );
    if dropped > 0 {
        eprintln!("WARNING: {dropped} invocation(s) had no slot and were not counted");
    }
    for (index, name) in Phase::NAMES.iter().enumerate() {
        let nanos = phase_nanos[index];
        let count = phase_calls[index];
        if count == 0 {
            continue;
        }
        eprintln!(
            "phase {name:<12} {:8.3} s  {count:>8} calls  {:8.1} us/call",
            nanos as f64 / 1e9,
            nanos as f64 / count as f64 / 1e3
        );
    }
    // The sum is CPU time over every worker, not elapsed time. Dividing it by the core count is
    // the closest it comes to a wall-clock figure, and even that ignores what the phases cost.
    eprintln!(
        "cops total {:.3} s of CPU over {calls} invocations ({:.1} us each)",
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
