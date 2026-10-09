use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-env-changed=GPM_RC_PATH");
    println!("cargo:rerun-if-env-changed=GPUI_FXC_PATH");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = root
        .join("assets/app-icon.ico")
        .to_string_lossy()
        .replace('\\', "/");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let source = format!(
        r#"
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION 0,1,0,0
PRODUCTVERSION 0,1,0,0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Project Manager\0"
      VALUE "ProductName", "Project Manager\0"
      VALUE "InternalName", "Project Manager\0"
      VALUE "OriginalFilename", "project-manager.exe\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "ProductVersion", "{version}\0"
      VALUE "LegalCopyright", "Copyright (c) 2026 Project Manager contributors\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let rc = output.join("project-manager.rc");
    let resource = output.join("project-manager.res");
    fs::write(&rc, source).expect("write Windows resources");
    let compiler = env::var_os("GPM_RC_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("GPUI_FXC_PATH")
                .and_then(|fxc| PathBuf::from(fxc).parent().map(|p| p.join("rc.exe")))
        })
        .unwrap_or_else(|| PathBuf::from("rc.exe"));
    let status = Command::new(compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&rc)
        .status()
        .expect("Windows SDK rc.exe is required; build with scripts/build-native.ps1");
    assert!(status.success(), "Windows resource compilation failed");
    println!("cargo:rustc-link-arg={}", resource.display());
}
