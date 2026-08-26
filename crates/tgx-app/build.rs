//! Embed the icon and the Windows version resource into `TelegramExporter.exe`.
//!
//! Without this the Properties dialog shows an exe with no product name, no
//! version and no copyright, and Explorer draws the generic binary glyph. The
//! icon is `icon/TelegramExporter.ico`, drawn by `tools/make_icon.py` from the
//! design tokens — regenerate it there rather than editing the file.
//!
//! **The binary is unsigned, and stays unsigned here.** An Authenticode
//! certificate costs money and an identity check, and this is a personal tool
//! distributed as a folder you copy. What it costs a user: SmartScreen
//! interposes a "Windows protected your PC" dialog the first time a *downloaded*
//! copy is run, needing "More info" then "Run anyway". A version resource does
//! not change that — reputation is keyed on the signature, not the metadata —
//! but an exe that at least names itself is the difference between a scary
//! unknown and a recognisable one. Copying `dist\` over the network or on a
//! stick does not attach the mark-of-the-web and does not trip SmartScreen at
//! all.

fn main() {
    // Two guards for two directions of cross-compilation. `cfg(windows)` here
    // is the host, and matches the Cargo.toml gate that decides whether the
    // crate exists; `CARGO_CFG_WINDOWS` is the target, and a Windows host
    // building for Linux has no PE to put a resource in.
    #[cfg(windows)]
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed();
    }
}

#[cfg(windows)]
fn embed() {
    println!("cargo:rerun-if-changed=icon/TelegramExporter.ico");
    println!("cargo:rerun-if-changed=build.rs");

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("icon/TelegramExporter.ico");
    // `WindowsResource::new` already fills FileVersion and ProductVersion from
    // CARGO_PKG_VERSION, which is where the version has to come from: two
    // places to change a version number is one place to forget one. The rest
    // are Cargo defaults only by accident of the package name, so state them.
    resource.set("ProductName", "Telegram Exporter");
    resource.set("FileDescription", "Telegram Exporter");
    resource.set("LegalCopyright", "Copyright (C) 2026 Kosta Jovanovic");
    resource.set("OriginalFilename", "TelegramExporter.exe");

    // Never fail the build over this. Compiling a resource needs `rc.exe` from
    // the Windows SDK (or the bundled `windres`), and a developer with a
    // toolchain but no SDK still has to be able to `cargo build`. The failure
    // being avoided is a cosmetic feature hard-failing an unrelated build: an
    // exe with no icon runs exactly as well as one with an icon, so this is a
    // warning and the build carries on.
    if let Err(e) = resource.compile() {
        println!("cargo:warning=no version resource or icon embedded: {e}");
    }
}
