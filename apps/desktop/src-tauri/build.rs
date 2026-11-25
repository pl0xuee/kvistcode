use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // Commit of this build for the diagnostics report (set it explicitly to override; a
    // source tree without git reports "unknown"). Not tracked for reruns: CI builds are clean.
    println!("cargo:rerun-if-env-changed=KVISTCODE_GIT_SHA");
    if std::env::var_os("KVISTCODE_GIT_SHA").is_none() {
        let sha = std::process::Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .current_dir(&manifest_dir)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit()));
        if let Some(sha) = sha {
            println!("cargo:rustc-env=KVISTCODE_GIT_SHA={sha}");
        }
    }

    // tauri-build links its Windows manifest (Common Controls v6) into the app binary
    // only; test binaries using `tauri::test` then die at load with
    // STATUS_ENTRYPOINT_NOT_FOUND. Have the linker embed the same manifest everywhere.
    let msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if msvc {
        let manifest = manifest_dir.join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
            .expect("tauri build");
    } else {
        tauri_build::build();
    }
}
