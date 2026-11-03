//! build.rs — embeds `patina.ico` and the version block in `Patina.exe`, so
//! Explorer, the taskbar pin, Alt+Tab and the installer's shortcuts show
//! Patina's icon, and Task Manager its name. Windows only; elsewhere a no-op.

fn main() {
    println!("cargo:rerun-if-changed=patina.ico");
    println!("cargo:rerun-if-changed=build.rs");

    // The target, not the host: `cfg(windows)` here would describe the
    // machine running the build script.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winres::WindowsResource::new();
    res.set_icon("patina.ico")
        .set("ProductName", "Patina")
        .set("FileDescription", "Patina");
    // A failed resource compile fails the build: an exe without its icon is
    // the bug this script exists to prevent, not something to ship quietly.
    if let Err(e) = res.compile() {
        panic!("embedding patina.ico failed (needs rc.exe from the Windows SDK): {e}");
    }
}
