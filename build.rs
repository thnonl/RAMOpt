fn main() {
    println!("cargo:rerun-if-changed=assets/ramopt.ico");
    slint_build::compile("src/main.slint").expect("Slint UI compilation failed");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/ramopt.ico");
        resource.set("ProductName", "RAMOpt");
        resource.set("FileDescription", "RAMOpt - lightweight Windows RAM cleanup");
        resource.compile().expect("failed to compile Windows resources");
    }
}
