use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-changed=data/stratagems");
    println!("cargo:rerun-if-changed=data/stratagems.json");

    embed_catalog_icons();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/app-icon.ico")
            .compile()
            .expect("failed to embed Windows application icon");
    }
}

/// Generates `catalog_icons.rs` so every icon file under `data/stratagems` is
/// embedded in the executable and looked up by file name at runtime.
fn embed_catalog_icons() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon_dir = manifest_dir.join("data").join("stratagems");
    let out_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("catalog_icons.rs");

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(dir) = fs::read_dir(&icon_dir) {
        for entry in dir.flatten() {
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase());
            if !matches!(extension.as_deref(), Some("svg") | Some("png")) {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
                entries.push((name.to_string(), path.clone()));
            }
        }
    }
    entries.sort();

    let mut source = String::from(
        "/// Catalog icon files embedded at build time, sorted by file name.\n\
         pub static CATALOG_ICON_FILES: &[(&str, &[u8])] = &[\n",
    );
    for (name, path) in &entries {
        source.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            name,
            path.to_string_lossy()
        ));
    }
    source.push_str("];\n");
    fs::write(&out_path, source).expect("failed to write catalog_icons.rs");
}
