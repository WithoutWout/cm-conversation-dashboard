fn main() {
    // `tauri_build::build()` generates the context that `tauri::generate_context!()`
    // expands, and assumes it is producing a desktop binary. The wasm build has no
    // Tauri host at all (see the target gates in lib.rs), so skip it there —
    // build scripts always run on the host, so this cannot be a `#[cfg]`.
    if std::env::var("TARGET").unwrap_or_default().starts_with("wasm32") {
        return;
    }
    tauri_build::build()
}
