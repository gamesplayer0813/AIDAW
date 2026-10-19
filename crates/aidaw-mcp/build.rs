//! Embed the AIDAW icon in aidaw-mcp.exe too, so it is easy to spot.

fn main() {
    let icon = "../aidaw-gui/assets/icon/aidaw.ico";
    println!("cargo:rerun-if-changed={icon}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon(icon)
            .set("FileDescription", "AIDAW MCP server")
            .set("ProductName", "AIDAW")
            .compile()
            .expect("embed the app icon");
    }
}
