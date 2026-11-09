use std::fs;

fn main() {
    // rust-embed requires the folder to exist at compile time. Creating it
    // here lets `cargo build` succeed before the UI has ever been built
    // (the server then serves a "UI not built" hint instead of assets).
    fs::create_dir_all("ui/dist").expect("failed to create ui/dist");
    println!("cargo:rerun-if-changed=ui/dist");
}
