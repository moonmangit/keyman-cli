//! Windows: embeds the app icon and version details into the executable, so Explorer, the
//! taskbar, shortcuts and Alt+Tab show Keyman's icon. Other targets need nothing here.

fn main() {
    println!("cargo:rerun-if-changed=assets/keyman.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // Resource 1 is the icon main() hands to Rune (set_app_icon_resource).
        res.set_icon_with_id("assets/keyman.ico", "1")
            .set("ProductName", "Keyman")
            .set("FileDescription", "Keyman")
            .set("CompanyName", "Keyman")
            .set("LegalCopyright", "MIT License");
        if let Err(error) = res.compile() {
            panic!("cannot embed the Windows icon: {error}");
        }
    }
}
