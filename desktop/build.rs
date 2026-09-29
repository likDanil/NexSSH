fn main() {
    explorer_package();
    tauri_build::build();
}

/// With `NEXSSH_EXPLORER_PACKAGE` (the folder `scripts/explorer-package.ps1` writes), the build
/// embeds the package of the Windows 11 menu entry, and `cfg(explorer_package)` is set.
fn explorer_package() {
    println!("cargo::rustc-check-cfg=cfg(explorer_package)");
    println!("cargo::rerun-if-env-changed=NEXSSH_EXPLORER_PACKAGE");
    let Some(dir) = std::env::var_os("NEXSSH_EXPLORER_PACKAGE").filter(|d| !d.is_empty()) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    // FNV-1a of the files: the app unpacks them into a folder named after the build.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for name in ["NexSSH.msix", "nexssh_explorer_command.dll", "logo.png"] {
        let path = dir.join(name);
        let data = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("NEXSSH_EXPLORER_PACKAGE: {}: {e}", path.display()));
        for byte in data {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        println!("cargo::rerun-if-changed={}", path.display());
    }
    println!("cargo::rustc-cfg=explorer_package");
    println!("cargo::rustc-env=NEXSSH_EXPLORER_PACKAGE={}", dir.display());
    println!(
        "cargo::rustc-env=NEXSSH_EXPLORER_PACKAGE_ID={}-{hash:016x}",
        env!("CARGO_PKG_VERSION")
    );
}
