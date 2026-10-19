//! Embed the app icon in aidaw.exe so Explorer and the taskbar show it.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/aidaw.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon/aidaw.ico")
            .set("FileDescription", "AIDAW")
            .set("ProductName", "AIDAW")
            .compile()
            .expect("embed the app icon");
    }
}
