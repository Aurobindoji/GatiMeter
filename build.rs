fn main() {
    println!("cargo:rerun-if-changed=res/gatimeter.ico");
    println!("cargo:rerun-if-changed=res/gatimeter.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("res/gatimeter.ico")
            .set_manifest_file("res/gatimeter.manifest")
            .set("ProductName", "GatiMeter")
            .set("FileDescription", "GatiMeter - network speed and system load on the taskbar")
            .set("LegalCopyright", "GPL-3.0-or-later")
            .set("OriginalFilename", "GatiMeter.exe");
        res.compile().expect("resource compile failed");
    }
}
