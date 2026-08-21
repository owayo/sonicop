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
/// It is reserved address space rather than memory: Linux and macOS commit stack pages when they
/// are first touched, so an ordinary file still costs the handful of pages it actually uses, and
/// the whole rayon pool on a 16-core host reserves a gigabyte of the 128 TiB a 64-bit process is
/// given. That is what makes 8x the 8 MiB the main thread came with affordable -- measured on a
/// debug build, it moves the deepest `1 + 1 + ...` chain that survives from ~3,600 terms to
/// ~29,000, well past anything a hand-written or generated Ruby file reaches, and a release build's
/// smaller frames go further still. Going another order of magnitude up would buy depth no real
/// file needs at the price of a reservation a 32-bit target could not satisfy.
const STACK_SIZE: usize = 64 * 1024 * 1024;

/// What the process exits with when a thread panics, which is what would have happened had the
/// panic been on the main thread. The default hook has already printed the message by then.
const PANIC: i32 = 101;

fn main() {
    // Rayon's workers get stacks of their own and know nothing about this thread's, so linting more
    // than one file would still abort on the one that goes deep -- and still lose every other
    // file's offenses with it. `build_global` is process-wide and may be called only once; nothing
    // else in the crate calls it, and if it does fail the pool keeps its default stack, which is no
    // worse off than never having asked.
    let _ = rayon::ThreadPoolBuilder::new()
        .stack_size(STACK_SIZE)
        .build_global();

    // A system that will not hand out a stack this size is still one the linter should run on, so
    // the fallback is the main thread the work used to get rather than an abort of its own.
    let code = match std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(sonicop::run)
    {
        Ok(linting) => linting.join().unwrap_or(PANIC),
        Err(_) => sonicop::run(),
    };

    std::process::exit(code);
}
