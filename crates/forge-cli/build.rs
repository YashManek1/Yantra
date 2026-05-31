//! Build script for the yantra CLI binary.
//!
//! On Windows (MSVC), embeds:
//! - A Windows application manifest (`yantra.manifest`) declaring the application
//!   identity, standard-user privilege level, long-path awareness, and Windows 10/11
//!   compatibility — so Windows correctly identifies the binary as a known application.
//! - Version-info resources (ProductName, FileDescription, LegalCopyright) visible in
//!   Explorer → Properties → Details and used by Windows Defender reputation tracking.
//!
//! On non-Windows targets the build script is a no-op.

fn main() {
    #[cfg(target_os = "windows")]
    embed_windows_resources();
}

#[cfg(target_os = "windows")]
fn embed_windows_resources() {
    let mut windows_resource = winresource::WindowsResource::new();
    windows_resource
        .set("ProductName", "Yantra")
        .set(
            "FileDescription",
            "Yantra — Rust-native agentic coding runtime",
        )
        .set(
            "LegalCopyright",
            "Copyright © 2026 Sankalp Systems. Apache-2.0.",
        )
        .set("CompanyName", "Sankalp Systems")
        .set_manifest_file("yantra.manifest");

    if let Err(resource_error) = windows_resource.compile() {
        eprintln!("cargo:warning=Windows resource embedding failed: {resource_error}");
    }
}
