// Try overriding abort() function, which is called by slog on panic. On emscripten trying to get backtrace from abort()
// fails. Unfortunately, overriding __assert_fail() and __assert_rtn() does not produce interesting backtraces with
// Imgui.

#[cfg(not(target_os = "emscripten"))]
use std::backtrace::Backtrace;

#[cfg(not(target_os = "emscripten"))]
#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    eprintln!("Aborted!\n{}", Backtrace::force_capture());

    std::process::exit(1);
}

/// Callbacks with `extern "C"` abort when panic is caught: https://doc.rust-lang.org/nomicon/ffi.html#ffi-and-unwinding
/// Use a guard to ignore the panic and output an error log.
pub fn ffi_guard<F: FnOnce() + std::panic::UnwindSafe>(what: &str, f: F) {
    if std::panic::catch_unwind(f).is_err() {
        eprintln!("=== Panic in {what} callback was ignored ===");
    }
}
