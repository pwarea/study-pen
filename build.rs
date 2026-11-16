fn main() {
    println!("cargo:rerun-if-changed=resources/study-pen.rc");
    println!("cargo:rerun-if-changed=resources/study-pen.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let number = format!(
        "{},{},{},0",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    );
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output folder"));
    std::fs::write(
        output.join("study-pen-version.h"),
        format!(
            "#define STUDY_PEN_VERSION_NUM {number}\n#define STUDY_PEN_VERSION_TEXT \"{version}\"\n"
        ),
    )
    .expect("Could not prepare Study Pen product metadata");
    embed_resource::compile("resources/study-pen.rc", embed_resource::NONE)
        .manifest_required()
        .expect("Could not embed the Study Pen icon and product metadata");
}
