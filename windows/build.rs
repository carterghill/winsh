fn main() {
    println!("cargo:rerun-if-changed=../../windows/winsh.rc");
    println!("cargo:rerun-if-changed=../../windows/winsh.ico");
    embed_resource::compile_for(
        "../../windows/winsh.rc",
        [env!("CARGO_PKG_NAME")],
        embed_resource::ParamsIncludeDirs(["../../windows"]),
    )
    .manifest_required()
    .expect("could not embed the winsh application icon");
}
