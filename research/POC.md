# R-01 至 R-06 技术预研 POC

本 POC 用于验证 Rust 工程基线、官方 `rmcp` tools-only Server、MCP 工具 Schema、Linux 安全文件访问、有界日志扫描，以及阻塞任务的并发和取消控制。

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
- 基于 `Read` 的有界流式日志扫描器。
- 跨读取缓冲区的字面量搜索。
- 中文 UTF-8、ASCII 大小写选项、非法 UTF-8 和 CRLF 处理。
- 超长单行的有限匹配窗口。
- 扫描字节、结果数量、单条内容和返回内容合计限制。
- 基于 `spawn_blocking` 的扫描执行器。
- 基于 Semaphore 的全局扫描并发限制。
- `CancellationToken` 和绝对 deadline 的协作检查。

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

这是预研 POC，不是正式日志查询服务：

- MCP 工具仍使用内存模拟记录，尚未接入真实 `SafeRoot + ScanExecutor` 查询链路。
- `SafeRoot` 只验证已知相对路径的安全打开，尚未实现目录发现和管理员文件名规则。
- 扫描器只负责单个 `Read`，尚未实现多文件和多日志来源编排。
- 返回内容合计限制不等于完整 MCP JSON 响应大小限制。
- 等待 Semaphore 许可期间尚未观察取消和 deadline。
- 尚未验证客户端断开能否从 `rmcp` / Axum 传递到扫描 CancellationToken。
- 尚未执行 1 GiB、10 GiB 和大量小文件性能基准。
- 尚未实现分页游标。
- 时间字段已进入 Schema，但尚未执行时间过滤。
- `match_ref` 仍为确定性测试值，不是安全的服务端状态引用。
- `openat2()` 方案要求 Linux kernel 5.6 及以上。
- MCP Inspector 和目标 AI 客户端兼容性仍需人工执行并记录结果。

## 对应文档

- [技术预研计划](../docs/TECHNICAL_RESEARCH_PLAN.md)
- [工具 Schema 草案](../docs/TOOL_SCHEMA_DRAFT.md)
- [安全文件访问预研](../docs/SAFE_FILE_ACCESS_RESEARCH.md)
- [有界日志扫描器预研](../docs/SCANNER_RESEARCH.md)
- [阻塞扫描执行器预研](../docs/EXECUTOR_RESEARCH.md)

下一阶段进入 R-07：设计安全的服务端有状态 `match_ref`，并将安全文件打开、扫描结果和上下文读取串联起来。
