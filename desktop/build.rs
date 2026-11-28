//! Compiles `app.rc` (icon and version information) with the Windows SDK's `rc.exe` and links
//! the result into the executable. No build dependency: `rc.exe` is found through `RC` or the
//! newest Windows Kits 10 folder. Without it the build still succeeds, with a warning, and the
//! executable has no icon.
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_rc() -> Option<PathBuf> {
    if let Some(rc) = env::var_os("RC") {
        return Some(PathBuf::from(rc));
    }
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(kits)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("x64").join("rc.exe").is_file())
        .collect();
    versions.sort();
    versions.pop().map(|v| v.join("x64").join("rc.exe"))
}

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-env-changed=RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found: the executable will have no icon");
        return;
    };
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("app.res");
    let status = Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&out)
        .arg("app.rc")
        .status()
        .expect("run rc.exe");
    assert!(status.success(), "rc.exe failed on app.rc");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
