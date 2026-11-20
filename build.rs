fn main() {
    println!("cargo:rerun-if-changed=multipapr.manifest");
    let mut resources = winresource::WindowsResource::new();
    resources.set_manifest(include_str!("multipapr.manifest"));
    resources
        .compile()
        .expect("embed Windows compatibility manifest");
}
