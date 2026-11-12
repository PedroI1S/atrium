//! `tauri::generate_context!` embeds the frontend build (`../dist`) and
//! refuses to compile without it. So that `cargo check|clippy|test` work
//! before any `pnpm build`, an empty placeholder page is written when the
//! folder is missing. A real build (`pnpm tauri build`, or `pnpm build`)
//! replaces it; the placeholder says what it is.

use std::path::Path;

const PLACEHOLDER: &str = "<!doctype html><title>Atrium</title><p>The frontend isn't built. Run <code>pnpm build</code> in <code>app/</code>.</p>\n";

fn main() {
    let dist = Path::new("../dist");
    if !dist.join("index.html").exists()
        && let Err(error) = std::fs::create_dir_all(dist)
            .and_then(|()| std::fs::write(dist.join("index.html"), PLACEHOLDER))
    {
        println!("cargo:warning=couldn't write a placeholder frontend: {error}");
    }
    tauri_build::build();
}
