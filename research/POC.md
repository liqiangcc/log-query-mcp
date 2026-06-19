# R-01 至 R-04 技术预研 POC

本 POC 用于验证 Rust 工程基线、官方 `rmcp` tools-only Server、MCP 工具 Schema，以及 Linux 安全文件打开方案。

## 已实现

- Rust 2024 工程，并在应用层启用 `#![forbid(unsafe_code)]`。
- 官方 `rmcp` SDK 的结构化输入和输出。
- Streamable HTTP 入口：`http://127.0.0.1:8000/mcp`。
- 可选 stdio 入口。
- 三个使用确定性内存数据的 MCP 工具：
  - `list_log_sources`
  - `search_logs`
  - `get_log_context`
- 工具 JSON Schema 中的数量、长度、枚举和默认值限制。
- 与 Schema 对应的服务端运行时校验。
- 小型跨服务日志样例。
- 基于 Linux `openat2()` 的安全文件打开 POC。
- 路径穿越、软链接、目录、FIFO、Unix Socket 和文件替换测试。

## 运行 Streamable HTTP

```bash
cargo run
```

仅在受控网络中测试远程监听：

```bash
LOG_QUERY_MCP_BIND=0.0.0.0:8000 cargo run
```

默认只监听本机回环地址。

## 运行 stdio

```bash
cargo run --bin log-query-mcp-stdio
```

## 使用 MCP Inspector

stdio：

```bash
npx @modelcontextprotocol/inspector cargo run --bin log-query-mcp-stdio
```

Streamable HTTP：

```text
http://127.0.0.1:8000/mcp
```

## 质量检查

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

## 当前边界

这是预研 POC，不是正式日志扫描服务：

- MCP 工具仍使用内存模拟记录，尚未接入真实文件扫描器。
- `SafeRoot` 只验证已知相对路径的安全打开，尚未实现目录发现和文件名规则。
- 尚未实现有界扫描缓冲区、扫描字节限制和响应大小限制。
- 尚未把取消和超时传递到阻塞扫描任务。
- 尚未实现分页游标。
- 时间字段已进入 Schema，但尚未执行时间过滤。
- `match_ref` 仍为确定性测试值，不是安全的服务端状态引用。
- `openat2()` 方案要求 Linux kernel 5.6 及以上。
- MCP Inspector 和目标 AI 客户端兼容性仍需人工执行并记录结果。

## 对应文档

- [技术预研计划](../docs/TECHNICAL_RESEARCH_PLAN.md)
- [工具 Schema 草案](../docs/TOOL_SCHEMA_DRAFT.md)
- [安全文件访问预研](../docs/SAFE_FILE_ACCESS_RESEARCH.md)

下一阶段进入 R-05 有界流式日志扫描器和 R-06 阻塞任务池、超时与取消传播。
