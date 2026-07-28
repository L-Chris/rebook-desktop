use std::env;
use std::path::PathBuf;

fn main() {
    let icon = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/windows/torto.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if !icon.exists() {
        println!("cargo:warning=Windows icon is missing; run the generate_windows_icons example");
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(
            icon.to_str()
                .expect("Windows icon path must be valid UTF-8"),
        )
        .set("ProductName", "Torto")
        .set("FileDescription", "Torto - 小龟阅读")
        .set("OriginalFilename", "torto.exe");
    resource
        .compile()
        .expect("failed to compile Windows resources");
}
