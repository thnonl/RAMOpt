fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/ramopt.ico");
    println!("cargo:rerun-if-env-changed=RAMOPT_EMBED_ADMIN_MANIFEST");
    slint_build::compile("src/main.slint").expect("Slint UI compilation failed");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let release_build = std::env::var("PROFILE").as_deref() == Ok("release");
        let embed_admin_manifest =
            release_build && std::env::var_os("RAMOPT_EMBED_ADMIN_MANIFEST").is_some();
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/ramopt.ico");
        resource.set("ProductName", "RAMOpt");
        resource.set(
            "FileDescription",
            "RAMOpt - lightweight Windows RAM cleanup",
        );
        if embed_admin_manifest {
            resource.set_manifest(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#,
            );
        }
        resource
            .compile()
            .expect("failed to compile Windows resources");
    }
}
