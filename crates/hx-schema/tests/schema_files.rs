//! The committed `schema/*.json` files are generated from the Rust types.
//! Run with `UPDATE_SCHEMA=1` to regenerate.

#[test]
fn committed_json_schemas_match_types() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema");
    let update = std::env::var_os("UPDATE_SCHEMA").is_some();
    for (name, schema) in hx_schema::json_schemas() {
        let path = dir.join(name);
        let want = serde_json::to_string_pretty(&schema).unwrap() + "\n";
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &want).unwrap();
            continue;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(have, want, "{name} is stale; rerun with UPDATE_SCHEMA=1");
    }
}
