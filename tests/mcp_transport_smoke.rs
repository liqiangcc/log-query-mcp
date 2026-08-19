use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::TempDir;

const CONFIG_ENV: &str = "LOG_QUERY_MCP_CONFIG";
const BIND_ENV: &str = "LOG_QUERY_MCP_BIND";

#[test]
fn stdio_binary_smoke_lists_tools() {
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_log-query-mcp-stdio"))
        .env(CONFIG_ENV, fixture.config_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stdio binary should start");

    let mut stdin = child.stdin.take().expect("stdin should be piped");
    let stdout = child.stdout.take().expect("stdout should be piped");
    let mut stdout = BufReader::new(stdout);

    write_json_line(&mut stdin, initialize_request(1));
    let initialized = read_json_response(&mut stdout, 1);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "log-query-mcp");

    write_json_line(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
    );
    write_json_line(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    );
    let tools = read_json_response(&mut stdout, 2);
    assert_tool_list_contains(&tools, "list_log_sources");
    assert_tool_list_contains(&tools, "search_logs");
    assert_tool_list_contains(&tools, "get_log_context");

    stop_child(child);
}

#[test]
fn http_binary_smoke_initializes_on_mcp_endpoint() {
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_log-query-mcp"))
        .env(CONFIG_ENV, fixture.config_path())
        .env(BIND_ENV, "127.0.0.1:0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("http binary should start");

    let stderr = child.stderr.take().expect("stderr should be piped");
    let mut stderr = BufReader::new(stderr);
    let mut line = String::new();
    stderr
        .read_line(&mut line)
        .expect("http binary should report listening address");
    let address = listening_address(&line);

    let response = post_json(&address, "/mcp", &initialize_request(1));
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"jsonrpc\":\"2.0\""), "{response}");
    assert!(response.contains("\"serverInfo\""), "{response}");
    assert!(response.contains("\"log-query-mcp\""), "{response}");

    stop_child(child);
}

struct Fixture {
    _root: TempDir,
    _config: TempDir,
    config_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("source root should be created");
        fs::write(
            root.path().join("application.log"),
            "2026-06-19T14:00:00+09:00 traceId=abc123 smoke\n",
        )
        .expect("log fixture should be written");

        let config = tempfile::tempdir().expect("config dir should be created");
        let config_path = config.path().join("config.json");
        fs::write(&config_path, config_json(root.path())).expect("config should be written");
        Self {
            _root: root,
            _config: config,
            config_path,
        }
    }

    fn config_path(&self) -> &Path {
        &self.config_path
    }
}

fn config_json(root: &Path) -> String {
    json!({
        "version": 1,
        "sources": [{
            "source_id": "payment-test",
            "name": "Payment test",
            "description": "payment-service application logs",
            "service": "payment-service",
            "environment": "test",
            "tags": ["payment", "java"],
            "enabled": true,
            "encoding": "utf-8",
            "root": root,
            "files": ["application.log"]
        }]
    })
    .to_string()
}

fn initialize_request(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {
                "name": "transport-smoke",
                "version": "0.2.0"
            }
        }
    })
}

fn write_json_line(stdin: &mut impl Write, value: Value) {
    writeln!(stdin, "{value}").expect("request should be written");
    stdin.flush().expect("request should be flushed");
}

fn read_json_response(stdout: &mut impl BufRead, id: u64) -> Value {
    for _ in 0..16 {
        let mut line = String::new();
        let bytes = stdout
            .read_line(&mut line)
            .expect("stdout should produce JSON-RPC lines");
        assert_ne!(bytes, 0, "server stdout closed before response {id}");
        let value: Value = serde_json::from_str(line.trim()).expect("stdout line should be JSON");
        if value["id"] == id {
            return value;
        }
    }
    panic!("response id {id} not found");
}

fn assert_tool_list_contains(response: &Value, tool_name: &str) {
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools/list response should contain tools");
    assert!(
        tools.iter().any(|tool| tool["name"] == tool_name),
        "tools/list should contain {tool_name}: {tools:?}"
    );
}

fn listening_address(line: &str) -> String {
    let marker = "listening on ";
    let start = line
        .find(marker)
        .unwrap_or_else(|| panic!("listening line missing address: {line:?}"))
        + marker.len();
    let end = line[start..]
        .find(' ')
        .map_or(line.len(), |offset| start + offset);
    line[start..end].to_owned()
}

fn post_json(address: &str, path: &str, body: &Value) -> String {
    let body = body.to_string();
    let mut stream = TcpStream::connect(address).expect("HTTP server should accept connections");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout should be set");
    write!(
        stream,
        concat!(
            "POST {path} HTTP/1.1\r\n",
            "Host: {address}\r\n",
            "Content-Type: application/json\r\n",
            "Accept: application/json, text/event-stream\r\n",
            "Connection: close\r\n",
            "Content-Length: {length}\r\n",
            "\r\n",
            "{body}"
        ),
        path = path,
        address = address,
        length = body.len(),
        body = body
    )
    .expect("HTTP request should be written");
    stream.flush().expect("HTTP request should be flushed");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("HTTP response should be readable");
    response
}

fn stop_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}
