fn main() {
    println!("cargo:rustc-env=RAMOPT_TAURI_BUILD=1");
    println!("cargo:rustc-env=RAMOPT_TAURI_HOST=1");
    #[cfg(windows)]
    let windows = tauri_build::WindowsAttributes::new()
        .window_icon_path("../assets/ramopt.ico")
        .app_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#,
        );
    let app_manifest = tauri_build::AppManifest::new().commands(&[
        "get_app_state",
        "save_settings",
        "restore_defaults",
        "clean_now",
        "hide_window",
        "install_update",
    ]);
    let attrs = tauri_build::Attributes::new().app_manifest(app_manifest);
    #[cfg(windows)]
    let attrs = attrs.windows_attributes(windows);
    tauri_build::try_build(attrs).expect("failed to run Tauri build script");
}
