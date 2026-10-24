use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_sqlite3_c() -> Option<PathBuf> {
    let local = PathBuf::from("vendor/sqlite3/sqlite3.c");
    if local.exists() {
        return Some(local);
    }
    let cargo_home = env::var("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|_| env::var("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .ok()?;
    let registry_src = cargo_home.join("registry/src");
    if let Ok(entries) = std::fs::read_dir(&registry_src) {
        for entry in entries.flatten() {
            let candidate = entry
                .path()
                .join("libsqlite3-sys-0.27.0/sqlite3/sqlite3.c");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
        let lib_a = out_dir.join("libsqlite3.a");
        if !lib_a.exists() {
            if let Some(sqlite_c) = find_sqlite3_c() {
                let obj_o = out_dir.join("sqlite3.o");
                let cc = env::var("CC_x86_64_pc_windows_gnu")
                    .unwrap_or_else(|_| "x86_64-w64-mingw32-gcc".to_string());
                let ar = env::var("AR_x86_64_pc_windows_gnu")
                    .unwrap_or_else(|_| "x86_64-w64-mingw32-ar".to_string());

                let status = Command::new(&cc)
                    .args([
                        "-O2",
                        "-DSQLITE_THREADSAFE=1",
                        "-DSQLITE_ENABLE_MATH_FUNCTIONS",
                        "-DSQLITE_ENABLE_JSON1",
                        "-c",
                    ])
                    .arg(&sqlite_c)
                    .arg("-o")
                    .arg(&obj_o)
                    .status()
                    .expect("Failed to invoke mingw gcc for sqlite3.c");
                assert!(status.success(), "Failed to compile sqlite3.c for Windows");

                let ar_status = Command::new(&ar)
                    .arg("rcs")
                    .arg(&lib_a)
                    .arg(&obj_o)
                    .status()
                    .expect("Failed to invoke mingw ar for libsqlite3.a");
                assert!(ar_status.success(), "Failed to archive libsqlite3.a");
            }
        }
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=sqlite3");
    } else {
        println!("cargo:rustc-link-lib=sqlite3");
    }
    if Path::new("build.rs").exists() {
        println!("cargo:rerun-if-changed=build.rs");
    }
}
