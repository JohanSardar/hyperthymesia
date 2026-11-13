use std::{env, fs, path::Path};

fn collect(root: &Path, dir: &Path, assets: &mut String) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(Result::unwrap).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink(), "web assets must not be symlinks");
        if kind.is_dir() {
            collect(root, &path, assets);
        } else if kind.is_file() {
            assets.push_str(&format!(
                "({:?}, include_bytes!({:?})),\n",
                path.strip_prefix(root).unwrap().to_str().unwrap(),
                path.to_str().unwrap()
            ));
        }
    }
}
fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/dist");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut assets = String::from("&[\n");
    if root.join("index.html").is_file() {
        collect(&root, &root, &mut assets);
    } else {
        assets.push_str("(\"index.html\", b\"<!doctype html><title>Hyperthymesia</title><h1>Hyperthymesia</h1><p>The web app build is not installed yet.</p>\"),\n");
    }
    assets.push(']');
    fs::write(
        Path::new(&env::var_os("OUT_DIR").unwrap()).join("app_assets.rs"),
        assets,
    )
    .unwrap();
}
