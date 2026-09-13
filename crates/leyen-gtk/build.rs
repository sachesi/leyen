use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let ui_src = data.join("ui");
    let compiled = PathBuf::from(env::var("OUT_DIR").unwrap()).join("resources");
    let ui_out = compiled.join("ui");
    fs::create_dir_all(&ui_out).unwrap();

    let mut blueprints: Vec<PathBuf> = fs::read_dir(&ui_src)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "blp"))
        .collect();
    blueprints.sort();
    let status = Command::new("blueprint-compiler")
        .arg("batch-compile")
        .arg(&ui_out)
        .arg(&ui_src)
        .args(&blueprints)
        .status()
        .expect("blueprint-compiler not found; install it (dnf install blueprint-compiler)");
    assert!(status.success(), "blueprint-compiler failed");

    // The compiled templates come from OUT_DIR, the stylesheet and the icons from data/.
    glib_build_tools::compile_resources(
        &[&compiled, &data],
        data.join("leyen.gresource.xml").to_str().unwrap(),
        "leyen.gresource",
    );

    for watched in ["ui", "icons", "leyen.gresource.xml", "style.css"] {
        println!("cargo:rerun-if-changed={}", data.join(watched).display());
    }
}
