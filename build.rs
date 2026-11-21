use sha2::{Digest, Sha256};
use std::{env, fs, io::Write, path::PathBuf, process::Command};

const MPV_BUILD: &str = "20261111";
const MPV_URL: &str = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-x86_64-20261111-git-bcde6b93cc.7z";
const MPV_SHA256: &str = "f259b5fd4b5c395617c948a4fadcdf55bba1c5282fb7c13e88626b3f71072baf";
const MPV_FILES: [&str; 2] = ["mpv.exe", "d3dcompiler_43.dll"];

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

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let mpv_cache = out_dir.ancestors().nth(3).unwrap().join("mpv-cache");
    fs::create_dir_all(&mpv_cache).unwrap();
    let bundle = mpv_cache.join(format!("mpv-{MPV_BUILD}.zst"));
    if !bundle.is_file() {
        let archive = mpv_cache.join("mpv.7z");
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
        let extracted = mpv_cache.join("mpv");
        let _ = fs::remove_dir_all(&extracted);
        sevenz_rust2::decompress_file(&archive, &extracted).expect("unpack mpv archive");

        let part = bundle.with_extension("part");
        let mut z = zstd::Encoder::new(fs::File::create(&part).unwrap(), 19).unwrap();
        z.multithread(std::thread::available_parallelism().map_or(1, |n| n.get() as u32))
            .unwrap();
        z.long_distance_matching(true).unwrap();
        z.window_log(27).unwrap();
        z.write_all(&(MPV_FILES.len() as u32).to_le_bytes())
            .unwrap();
        for name in MPV_FILES {
            let data = fs::read(extracted.join(name)).expect("bundled mpv file");
            z.write_all(&(name.len() as u32).to_le_bytes()).unwrap();
            z.write_all(name.as_bytes()).unwrap();
            z.write_all(&(data.len() as u64).to_le_bytes()).unwrap();
            z.write_all(&data).unwrap();
        }
        z.finish().unwrap();
        fs::rename(&part, &bundle).unwrap();
    }

    let mut resources = winresource::WindowsResource::new();
    resources.set_manifest(include_str!("multipapr.manifest"));
    resources.set_icon("multipapr.ico");
    resources.append_rc_content(&format!(
        "101 RCDATA \"{}\"",
        bundle.display().to_string().replace('\\', "\\\\")
    ));
    resources
        .compile()
        .expect("embed Multipapr's Windows resources");
}
