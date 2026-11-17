// SPDX-License-Identifier: MPL-2.0
use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
            root.join("assets/app.manifest").display()
        );
    } else {
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("app-resource.o");
        let compiler = env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
        let status = Command::new(&compiler)
            .current_dir(&root)
            .args(["-I", "assets", "assets/app.rc", "-O", "coff", "-o"])
            .arg(&out)
            .status()
            .expect(
                "Windows resource compiler unavailable; install MinGW windres or use Windows MSVC",
            );
        assert!(status.success(), "Windows resource compilation failed");
        println!("cargo:rustc-link-arg={}", out.display());
    }
}
