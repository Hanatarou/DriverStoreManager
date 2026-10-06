use embed_manifest::manifest::{DpiAwareness, ExecutionLevel};
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    // Only embed the manifest and the resources when building for Windows.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // requireAdministrator: the program changes the Driver Store, so Windows asks for elevation at start.
        // Common Controls v6 is part of the default manifest (visual styles, ListView groups).
        // System DPI awareness: the window is laid out in 96-dpi pixels and scaled by the system DPI.
        let manifest = new_manifest("DriverStoreManager")
            .requested_execution_level(ExecutionLevel::RequireAdministrator)
            .dpi_awareness(DpiAwareness::System);
        embed_manifest(manifest).expect("unable to embed the application manifest");

        // The icon (resource 1) and the version information shown in the file properties.
        // Needs the resource compiler: rc.exe (MSVC) or windres (MinGW). DSM_SKIP_RESOURCES=1 skips it, only
        // meant for checking the code on a machine without one (the exe then has no icon).
        if std::env::var_os("DSM_SKIP_RESOURCES").is_none() {
            embed_resource::compile("resources/resources.rc", embed_resource::NONE)
                .manifest_required()
                .expect("unable to embed the icon and version information (resources/resources.rc)");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/resources.rc");
    println!("cargo:rerun-if-changed=resources/icon.ico");
}
