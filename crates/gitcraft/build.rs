fn main() -> std::io::Result<()> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Explorer and shortcuts read the executable's icon before the window exists.
        let icon = "../../icons/icon.ico";
        println!("cargo:rerun-if-changed={icon}");
        winresource::WindowsResource::new()
            .set_icon(icon)
            .set("ProductName", "GitCraft")
            .compile()?;
    }
    Ok(())
}
