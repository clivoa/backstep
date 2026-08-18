//! Compiles the libretro log shim.
//!
//! One C file, because `retro_log_printf_t` is variadic and stable Rust cannot
//! define a variadic `extern "C"` function. See `src/log_shim.c` for why the
//! log channel is worth a C dependency at all.

fn main() {
    println!("cargo:rerun-if-changed=src/log_shim.c");
    cc::Build::new()
        .file("src/log_shim.c")
        .warnings(true)
        .compile("rollback_log_shim");
}
