# R-01 至 R-07 技术预研 POC

本 POC 用于验证 Rust 工程基线、官方 `rmcp` tools-only Server、MCP 工具 Schema、Linux 安全文件访问、有界日志扫描、阻塞任务控制，以及短期不透明匹配引用与受控上下文读取。

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
- 排队阶段和运行阶段的 `CancellationToken` 与绝对 deadline 检查。
- async 扫描 Future 被中止后，协作取消阻塞任务并释放许可。
- 不可预测且带 TTL、容量上限的服务端 `match_ref`。
- 引用绑定日志来源、规范化相对路径、文件身份和真实扫描偏移。
- 文件轮转、截断和同 inode 内容改写检测。
- 有界向后查找和向前读取日志上下文。
- 安全打开、扫描、引用存储和上下文读取的完整集成测试。

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

R-01 至 R-07 当前代码已通过上述严格 CI。

## 当前边界

这是预研 POC，不是正式日志查询服务：

- MCP 三个工具仍使用内存模拟记录，尚未接入真实 `SafeRoot + ScanExecutor + MatchReferenceStore` 查询链路。
- `SafeRoot` 只验证已知相对路径的安全打开，尚未实现目录发现和管理员文件名规则。
- 扫描器只负责单个 `Read`，尚未实现多文件和多日志来源编排。
- 返回内容合计限制不等于完整 MCP JSON 响应大小限制。
- 尚未验证客户端断开能否从 `rmcp` / Axum 传递到扫描 `CancellationToken`。
- 上下文读取尚未接入取消和 deadline。
- 尚未执行 1 GiB、10 GiB 和大量小文件性能基准。
- 尚未实现分页游标。
- 时间字段已进入 Schema，但尚未执行时间过滤。
- `match_ref` Store 尚未成为 MCP Server 的共享状态。
- 首期引用为单实例内存状态，服务重启后失效。
- `openat2()` 方案要求 Linux kernel 5.6 及以上。
- MCP Inspector 和目标 AI 客户端兼容性仍需人工执行并记录结果。

## 对应文档

- [技术预研计划](../docs/TECHNICAL_RESEARCH_PLAN.md)
- [工具 Schema 草案](../docs/TOOL_SCHEMA_DRAFT.md)
- [安全文件访问预研](../docs/SAFE_FILE_ACCESS_RESEARCH.md)
- [有界日志扫描器预研](../docs/SCANNER_RESEARCH.md)
- [阻塞扫描执行器预研](../docs/EXECUTOR_RESEARCH.md)
- [安全 match_ref 与上下文读取预研](../docs/MATCH_REFERENCE_RESEARCH.md)

下一阶段应把已验证的组件接入真实 MCP 工具：配置一个受控日志来源，通过 `search_logs` 安全扫描真实文件并生成 `match_ref`，再由 `get_log_context` 解析引用并读取有限上下文。
