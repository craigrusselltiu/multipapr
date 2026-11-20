fn main() {
    println!("cargo:rerun-if-changed=multipapr.manifest");
    println!("cargo:rerun-if-changed=multipapr.ico");
    let mut resources = winresource::WindowsResource::new();
    resources.set_manifest(include_str!("multipapr.manifest"));
    resources.set_icon("multipapr.ico");
    resources
        .compile()
        .expect("embed Multipapr's Windows resources");
}
