//! Export the actual run-scoped MCP tool schemas without state or a model call.
fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "window": hx_server::mcp::tools(&hx_server::mcp::RunMode::Window),
            "setup": hx_server::mcp::tools(&hx_server::mcp::RunMode::Setup {
                source: "00000000000000000000000000".into()
            })
        }))
        .expect("tool schemas are JSON")
    );
}
