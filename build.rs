fn main() {
    println!("cargo:rerun-if-changed=icon.rc");
    println!("cargo:rerun-if-changed=icon.ico");

    // The icon is cosmetic, so a machine without a resource compiler should
    // still get a working binary rather than a failed build.
    if let Err(err) = embed_resource::compile("icon.rc", embed_resource::NONE).manifest_optional() {
        println!("cargo:warning={err}");
    }
}
