//! Embeds the application icon and version info into the Windows .exe.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "NotroDNS");
        res.set("FileDescription", "NotroDNS - DNS manager");
        // A missing resource compiler must not break the build; the app
        // just ships without an embedded exe icon in that case.
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
