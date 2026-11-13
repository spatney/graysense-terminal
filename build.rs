//! Embeds the app icon and version info in graysense.exe, so File Explorer, the taskbar, Start
//! menu shortcuts and Task Manager show Graysense instead of a generic program.

fn main() {
    println!("cargo:rerun-if-changed=assets/graysense.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/graysense.ico").set("FileDescription", "Graysense").set("ProductName", "Graysense").set("CompanyName", "Graysense");
    if let Err(e) = res.compile() {
        // Without the Windows SDK's resource compiler the app still builds, just with a generic icon.
        println!("cargo:warning=Couldn't embed the app icon ({e}). Install the Windows SDK to fix this.");
    }
}
