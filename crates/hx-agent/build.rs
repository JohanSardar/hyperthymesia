fn main() {
    if std::env::var_os("CARGO_FEATURE_E2E_HARNESS").is_some()
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        panic!("e2e-harness must never be enabled in release builds");
    }
}
