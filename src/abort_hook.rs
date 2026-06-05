// Try overriding abort() function, which is called by slog on panic. On emscripten trying to get backtrace from abort()
// fails. Unfortunately, overriding __assert_fail() and __assert_rtn() does not produce interesting backtraces with
// Imgui.

#[cfg(not(target_os = "emscripten"))]
use std::backtrace::Backtrace;

#[cfg(not(target_os = "emscripten"))]
#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    eprintln!("Aborted!\n{}", Backtrace::capture());

    std::process::exit(1);
}
