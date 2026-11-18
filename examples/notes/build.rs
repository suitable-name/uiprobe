//! Compiles the UI. The `ui-harness` feature switches Slint debug info on, which the harness's
//! element queries need; without the feature the compiler's default applies, which honours
//! `SLINT_EMIT_DEBUG_INFO` (this workspace's `.cargo/config.toml` sets it, so a plain
//! `cargo test --workspace` works here too). slint-build re-runs on that variable by itself.

fn main() {
    let mut config = slint_build::CompilerConfiguration::new().with_edition((1, 19));
    if std::env::var_os("CARGO_FEATURE_UI_HARNESS").is_some() {
        config = config.with_debug_info(true);
    }
    slint_build::compile_with_config("ui/notes.slint", config).expect("the notes UI compiles");
}
