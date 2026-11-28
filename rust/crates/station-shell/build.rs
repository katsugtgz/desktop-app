fn main() {
    // Embed the exe's icon resource (copied to rust/assets/station.ico; the original
    // packages/app/build/icon.ico is deleted with the TS tree) on Windows only: the window icon
    // is set at runtime from the same file (`platform_icon.rs`); this only fixes the
    // Explorer/taskbar fallback to the generic exe icon.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=station.rc");
        println!("cargo:rerun-if-changed=../../assets/station.ico");
        embed_resource::compile("station.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("failed to compile station.rc icon resource");
    }
}
