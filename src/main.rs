//! The entry point exists to choose the stack the linter runs on, and nothing else.
//!
//! Cops walk the tree by recursion -- `Lint/UselessAssignment` descends a frame per level of an
//! expression -- and Ruby puts no limit on how deep an expression goes. A stack overflow in Rust is
//! not an error a caller can catch: the runtime aborts the process. So a single generated file of
//! `x = 1 + 1 + ...` used to kill the whole run before one offense reached the formatter, taking the
//! results of every other file on the command line with it and printing nothing at all -- where
//! RuboCop raises `SystemStackError`, reports it and exits 1.
//!
//! Sizing the stack here guards every recursive cop at once, which rewriting one walk into a loop
//! would not; a deep enough file would just find the next cop that recurses.

/// The stack every part of the run gets, both the thread the work is handed to and the pool it
/// spreads over.
///
/// It is reserved address space rather than memory -- Linux and macOS commit stack pages when they
/// are first touched -- so an ordinary file still costs the handful of pages it actually uses, and
/// a whole 16-worker pool reserves a gigabyte of the 128 TiB a 64-bit process is given. That is
/// what makes 8x the 8 MiB the main thread came with affordable. What it buys, measured on the
/// deepest `x = 1 + 1 + ...` chain `Lint/UselessAssignment` can walk without aborting:
///
/// | where the walk ran | before | after |
/// |---|---|---|
/// | one file, debug build | 3,665 terms | 29,698 |
/// | two files, debug build -- a rayon worker's 2 MiB | 898 terms | 29,657 |
/// | one file, release build | 9,249 terms | 8x that, the stack being 8x |
///
/// Well past what a hand-written or generated Ruby file reaches, and the two paths now fail at the
/// same depth rather than the parallel one giving out first. Another order of magnitude would buy
/// depth no real file needs, at the price of a reservation a 32-bit target could not satisfy.
const STACK_SIZE: usize = 64 * 1024 * 1024;

/// What the process exits with when a thread panics, which is what would have happened had the
/// panic been on the main thread. The default hook has already printed the message by then.
const PANIC: i32 = 101;

fn main() {
    size_the_pool();
    std::process::exit(on_a_large_stack(sonicop::run));
}

/// Gives every rayon worker [`STACK_SIZE`] as well.
///
/// Workers get stacks of their own and know nothing about the one the work below is handed, so
/// linting more than one file would still abort on the file that goes deep -- and still lose every
/// other file's offenses with it. `build_global` is process-wide and may be called only once;
/// nothing else in the crate calls it, and a failure here leaves the pool on its default stack,
/// which is no worse off than never having asked.
fn size_the_pool() {
    let _ = rayon::ThreadPoolBuilder::new()
        .stack_size(STACK_SIZE)
        .build_global();
}

/// Runs `work` on a thread of its own, so that it gets [`STACK_SIZE`] rather than whatever the
/// process happened to be started with, and reports what it returned.
///
/// A system that will not hand out a stack this size is still one the linter should run on, so the
/// fallback is the calling thread -- what the work used to get -- and not an abort of its own.
fn on_a_large_stack(work: fn() -> i32) -> i32 {
    match std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(work)
    {
        Ok(linting) => linting.join().unwrap_or(PANIC),
        Err(_) => work(),
    }
}

/// Both halves of the fix are tested by walking deeper than the stack they replace could hold,
/// through the same two functions `main` calls -- which is also what keeps `main` calling them,
/// since a function only this module's tests reached would be dead code and `-D warnings` says so.
///
/// A stack overflow cannot be caught, so a regression aborts this test binary rather than failing
/// an assertion. That is the complaint the fix answers, reproduced.
#[cfg(test)]
mod tests {
    /// Bytes every frame of the walk is made to hold, so that a number of frames reads as a stack
    /// size. `black_box` is what stops the optimiser noticing the array is dead and leaving the
    /// frame empty.
    const FRAME: usize = 1024;

    /// Frames enough to want 4 MiB of stack: twice what a spawned thread and a rayon worker are
    /// each given when nobody asks, so a walk this deep only returns on a stack that was asked for.
    const FRAMES: usize = 4 * 1024 * 1024 / FRAME;

    fn descend(frames: usize) -> u8 {
        let mut frame = std::hint::black_box([0u8; FRAME]);
        if frames == 0 {
            return frame[0];
        }
        frame[0] = descend(frames - 1);
        std::hint::black_box(frame)[0]
    }

    #[test]
    fn the_work_runs_on_a_stack_larger_than_a_thread_is_given_by_default() {
        assert_eq!(super::on_a_large_stack(|| i32::from(descend(FRAMES))), 0);
    }

    /// **Sizing the thread the work runs on is only half of it.** With the pool left at its default
    /// the run aborted after 898 terms as soon as a second file on the command line put the walk on
    /// a worker -- worse than the 3,665 it managed on the 8 MiB main thread it started from, and
    /// with no output for either file. `broadcast` is what puts this walk on the workers themselves
    /// rather than on the thread that asks for it.
    #[test]
    fn every_pool_worker_gets_one_too() {
        super::size_the_pool();

        let walked = rayon::broadcast(|_| descend(FRAMES));

        assert_eq!(walked.len(), rayon::current_num_threads());
    }
}
