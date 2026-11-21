#[path = "../explorer/build_support.rs"]
mod explorer_package;
fn main() {
    explorer_package::embed(env!("CARGO_PKG_VERSION"));
    tauri_build::build();
}
