use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf, process::Command};

// Pinned portable mpv build, embedded in multipapr.exe and unpacked on first use.
const MPV_BUILD: &str = "20261111";
const MPV_URL: &str = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-x86_64-20261111-git-bcde6b93cc.7z";
const MPV_SHA256: &str = "f259b5fd4b5c395617c948a4fadcdf55bba1c5282fb7c13e88626b3f71072baf";

fn sha256(path: &PathBuf) -> String {
    let bytes = fs::read(path).unwrap_or_default();
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn main() {
    println!("cargo:rerun-if-changed=multipapr.manifest");
    println!("cargo:rerun-if-changed=multipapr.ico");
    println!("cargo:rustc-env=MPV_BUILD={MPV_BUILD}");

    let archive = PathBuf::from(env::var("OUT_DIR").unwrap()).join("mpv.7z");
    if sha256(&archive) != MPV_SHA256 {
        let part = archive.with_extension("part");
        let ok = Command::new("curl.exe")
            .args(["-fsSL", "--retry", "3", "-o"])
            .arg(&part)
            .arg(MPV_URL)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "download mpv from {MPV_URL}");
        assert_eq!(sha256(&part), MPV_SHA256, "mpv archive checksum mismatch");
        fs::rename(&part, &archive).unwrap();
    }

    let mut resources = winresource::WindowsResource::new();
    resources.set_manifest(include_str!("multipapr.manifest"));
    resources.set_icon("multipapr.ico");
    // Embedded as a resource: include_bytes! on 33 MB makes rustc very slow.
    resources.append_rc_content(&format!(
        "101 RCDATA \"{}\"",
        archive.display().to_string().replace('\\', "\\\\")
    ));
    resources
        .compile()
        .expect("embed Multipapr's Windows resources");
}
