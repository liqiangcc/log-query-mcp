use log_query_mcp::{AppConfig, AppConfigV2};

#[test]
fn shipped_v1_example_remains_valid() {
    AppConfig::from_json_str(include_str!("../examples/log-query-mcp.v1.json"))
        .expect("shipped v1 example must remain valid");
}

#[test]
fn shipped_v2_remote_example_remains_valid() {
    AppConfigV2::from_json_str(include_str!("../examples/log-query-mcp.v2.remote.json"))
        .expect("shipped v2 Remote example must remain valid");
}
