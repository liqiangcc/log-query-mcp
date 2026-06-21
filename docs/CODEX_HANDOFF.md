# Codex 交接说明：完成 Log Query MCP v1 服务

> 仓库：`liqiangcc/log-query-mcp`  
> 起始分支：最新 `main`  
> 当前阶段：M2 核心能力完成，待 MCP Server、错误边界、响应大小限制和正式传输入口  
> 工作方式：一次一个 PR；每个 PR 严格 CI 通过后再进入下一项

---

## 1. 给 Codex 的主提示词

将下面整段直接交给 Codex：

```text
你正在继续开发私有 GitHub 仓库：

liqiangcc/log-query-mcp

必须从最新 main 开始，不得基于旧 feature 分支、spike/technical-research 或已关闭/被 supersede 的 PR 继续开发。

开始前执行：

1. checkout main
2. pull / sync 到远端最新 main
3. 确认 git status 为空
4. 阅读：
   - README.md
   - REQUIREMENTS.md
   - docs/M2_STATUS.md
   - docs/IMPLEMENTATION_BASELINE_V1.md
   - docs/MCP_API_V1.md
   - docs/ERROR_MODEL_V1.md
   - docs/CONFIG_SCHEMA_V1.md
   - schemas/mcp-tools-v1.schema.json
   - schemas/tool-error-v1.schema.json
   - schemas/log-query-mcp-config-v1.schema.json
   - docs/CODEX_HANDOFF.md
5. 阅读当前公共 Rust API：
   - src/lib.rs
   - src/stateful_query.rs
   - src/stateful_context.rs
   - src/query_state.rs
   - src/source_registry.rs
   - src/config.rs
6. 运行当前基线：
   - cargo fmt --all -- --check
   - cargo clippy --locked --all-targets --all-features -- -D warnings
   - cargo test --locked --all-targets --all-features
   - python scripts/validate_contracts.py

当前 main 已完成：

- v1 版本化配置模型
- SourceRegistry / SafeRoot / openat2 / RESOLVE_NO_XDEV
- 有界目录发现和文件身份快照
- 有界流式扫描器和 ScanExecutor
- 多来源查询、[start_time, end_time) 时间过滤和 oldest_first 稳定排序
- StatefulQueryService
- SearchCursorStore
- MatchReferenceStore
- StatefulContextService
- 有界前后文读取

剩余目标：

1. v1 工具错误 wire format 和完整响应大小限制
2. rmcp Server 与三个工具：
   - list_log_sources
   - search_logs
   - get_log_context
3. stdio 和 Streamable HTTP 正式入口
4. MCP 协议级和端到端集成测试
5. README / 部署说明更新

必须拆成独立 PR，按以下顺序执行。不要一次提交全部工作：

PR A: feat/mcp-error-response-boundary
PR B: feat/mcp-server-tools
PR C: feat/mcp-transports-validation

每个 PR：

- 从合并后的最新 main 新建分支
- 不修改冻结的 v1 契约，除非发现明确矛盾；遇到矛盾应停止并在 PR 中说明
- 不自行合并 PR
- 必须通过 rustfmt、Clippy、全量测试和 contracts 校验
- PR 描述必须列出：实现内容、测试、未完成项、安全影响

绝对禁止：

- 客户端提交服务器路径、目录、glob 或任意行号
- 绕过 SourceRegistry / SafeRoot / openat2
- 调用 Shell、grep、rg、find 等外部命令处理日志
- 改为正则搜索
- 实现 newest_first
- 改变 [start_time, end_time) 语义
- 将非 ASCII 搜索改成 Unicode case folding；v1 仅保证 ASCII 大小写折叠
- 暴露绝对路径、inode、配置路径、系统调用参数、Rust backtrace 或凭证
- 把 cursor / match_ref 编码成客户端可解析的路径或偏移
- 引入 unsafe
- 直接复制 spike 分支旧实现而不适配当前 main
- 使用 rmcp 仓库主分支或未发布 git revision；使用已发布稳定版本并提交 Cargo.lock

当三个 PR 全部完成后，输出：

- 合并顺序
- 每个 PR URL
- 最终运行命令
- MCP Inspector 验证步骤
- 仍需在目标 Linux 服务器完成的验收项
```

---

## 2. 当前代码基线

最新 `main` 已完成六个正式切片：

1. 版本化配置模型。
2. `SourceRegistry`、`SafeRoot`、`openat2()` 和受控目录发现。
3. 有界扫描器、`spawn_blocking`、Semaphore、deadline 和取消。
4. 多来源查询、时间过滤和稳定排序。
5. 服务端 cursor、累计配额、排序水位线和 `match_ref`。
6. 通过 `match_ref` 的有限上下文读取。

关键公共 API：

```rust
AppConfig::load(...)
SourceRegistry::from_config(...)
StatefulQueryService::new(Arc<SourceRegistry>)
StatefulQueryService::search(StatefulQueryRequest)
StatefulContextService::from_query_service(&StatefulQueryService)
StatefulContextService::get_context(StatefulContextRequest)
```

查询结果类型：

```rust
StatefulQueryPage
RegisteredQueryMatch
StatefulQuerySummary
```

上下文结果类型：

```rust
StatefulContextResult
ContextLine
```

不要重新实现这些能力。MCP 层应当是薄适配层。

---

## 3. 忽略的历史内容

Codex 只能以最新 `main` 为事实来源。

不要从以下内容恢复代码：

- `spike/technical-research`
- 已关闭且标题包含 `superseded` 的 PR
- PR #13 及其他基于旧主线的重复实现
- 临时 bootstrap / fixup 工作流

可以阅读历史文档理解设计原因，但不得把旧实现直接覆盖到正式模块。

---

# PR A：错误边界与完整响应大小限制

## 4. 分支

```text
feat/mcp-error-response-boundary
```

## 5. 目标

在引入 rmcp 工具之前，先建立统一、可测试、与传输无关的错误和序列化边界。

建议新增：

```text
src/tool_error.rs
src/response_limit.rs
tests/tool_error_contract.rs
```

文件名可以调整，但职责必须独立。

## 6. `ToolError` 类型

实现与 `schemas/tool-error-v1.schema.json` 一致的类型：

```rust
pub enum ToolErrorCode {
    InvalidArgument,
    UnknownSource,
    SourceUnavailable,
    DeadlineExceeded,
    QueryCancelled,
    ResourceLimit,
    CursorInvalid,
    MatchRefInvalid,
    FileChanged,
    InternalError,
}

pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
    pub retryable: bool,
}
```

序列化代码必须是冻结的 SCREAMING_SNAKE_CASE：

```text
INVALID_ARGUMENT
UNKNOWN_SOURCE
SOURCE_UNAVAILABLE
DEADLINE_EXCEEDED
QUERY_CANCELLED
RESOURCE_LIMIT
CURSOR_INVALID
MATCH_REF_INVALID
FILE_CHANGED
INTERNAL_ERROR
```

禁止添加 `details`、`path`、`cause`、`backtrace` 等外部字段。

## 7. 错误消息要求

客户端消息：

- 使用简短英文。
- 不直接使用底层错误的 `Display` 输出。
- 不包含绝对路径。
- 不包含设备号、inode 或偏移。
- 不包含 OS 用户、配置位置或系统调用参数。
- 客户端依赖 `code`，不能依赖完整 `message`。

服务端可在后续 tracing 中记录内部原因，但不得进入 MCP 工具结果。

## 8. 必须实现的错误映射

实现独立映射函数或 `From`，但必须可单元测试且穷尽主要错误族。

### `StatefulQueryError`

建议映射：

| 内部错误 | v1 code |
|---|---|
| `InvalidArgument`、`TimeFilter` 参数错误 | `INVALID_ARGUMENT` |
| `Cancelled` | `QUERY_CANCELLED` |
| `DeadlineExceeded`、`DeadlineOverflow` | `DEADLINE_EXCEEDED` 或不可恢复的 overflow 映射 `INTERNAL_ERROR`；必须写测试固定选择 |
| `FileLimitExceeded`、`ResourceCounterOverflow`、累计资源限制 | `RESOURCE_LIMIT` |
| cursor `UnknownOrExpired`、`QueryMismatch`、`Busy`、`LeaseLost`、无效 continuation | `CURSOR_INVALID` |
| `SourceRegistry::UnknownSource` | `UNKNOWN_SOURCE` |
| 已配置文件不可安全读取、权限、缺失、NO_XDEV | `SOURCE_UNAVAILABLE` |
| device/inode、大小或扫描位置表明文件变化 | `FILE_CHANGED` |
| `ScanTask` 取消或 deadline | 对应 `QUERY_CANCELLED` / `DEADLINE_EXCEEDED` |
| 未预期 I/O、Join、状态不变量 | `INTERNAL_ERROR` |

### `StatefulContextError`

建议映射：

| 内部错误 | v1 code |
|---|---|
| `InvalidArgument` | `INVALID_ARGUMENT` |
| `Cancelled` | `QUERY_CANCELLED` |
| `DeadlineExceeded` | `DEADLINE_EXCEEDED` |
| match_ref `UnknownOrExpired` | `MATCH_REF_INVALID` |
| 来源不存在 | `UNKNOWN_SOURCE` |
| 上下文资源限制 | `RESOURCE_LIMIT` |
| 文件轮转、替换、截断、关键字复核失败 | `FILE_CHANGED` |
| 其他内部执行错误 | `INTERNAL_ERROR` |

必须检查当前错误枚举的真实 variant，不要只依赖本文表格猜测名称。

## 9. 错误 wire JSON

实现：

```rust
fn to_wire_json(&self) -> Result<String, ...>
```

或等价接口，输出紧凑 JSON：

```json
{"code":"CURSOR_INVALID","message":"the search cursor is invalid or expired; run the search again","retryable":false}
```

业务错误后续将作为：

```text
CallToolResult.isError = true
第一个 text content = 上述紧凑 JSON
```

PR A 不必依赖 rmcp，但输出必须可被 PR B 直接使用。

## 10. 完整响应大小限制

实现通用序列化边界，例如：

```rust
pub fn serialize_with_limit<T: Serialize>(
    value: &T,
    max_bytes: usize,
) -> Result<Vec<u8>, ResponseLimitError>
```

要求：

- 检查完整 UTF-8 JSON 字节数，不是只统计日志 `content`。
- 使用紧凑 JSON，不做 pretty print。
- 超过 `max_response_bytes` 时返回明确错误。
- 最终由 MCP 层映射为 `RESOURCE_LIMIT`。
- 不得在工具层简单删除已经返回页面中的前几条/后几条结果后继续沿用原 cursor，因为这可能跳过未发送结果。
- v1 优先策略：无法安全保持分页语义时，返回 `RESOURCE_LIMIT`，不要静默丢数据。
- 错误结果本身也应有保守固定上限，但不应递归产生另一个超限错误。

## 11. PR A 测试

至少覆盖：

1. 十个错误代码的精确 JSON 值。
2. `retryable` 与 `docs/ERROR_MODEL_V1.md` 一致。
3. 错误 JSON 没有额外字段。
4. 底层路径文本不会进入客户端错误。
5. 已知查询错误、cursor 错误、match_ref 错误和文件变化映射正确。
6. 小响应可序列化。
7. 恰好等于上限可通过。
8. 超过一个字节被拒绝。
9. UTF-8 多字节内容按字节数计算。

## 12. PR A 验收

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
python scripts/validate_contracts.py
```

通过后创建 PR，停止，等待合并。

---

# PR B：rmcp Server 与三个工具

## 13. 分支

从 PR A 合并后的最新 `main` 创建：

```text
feat/mcp-server-tools
```

## 14. 依赖要求

- 使用官方、已发布的稳定 `rmcp` crate。
- 不使用 git main、fork 或未发布 revision。
- 只启用实际需要的 server、macros、schemars 和传输无关能力。
- 提交更新后的 `Cargo.lock`。
- 遵循当前版本官方 API，不照搬 spike 分支旧版本写法。

建议新增模块：

```text
src/mcp_model.rs
src/mcp_server.rs
```

## 15. `LogQueryServer` 共享状态

建议结构：

```rust
pub struct LogQueryServer {
    registry: Arc<SourceRegistry>,
    query_service: Arc<StatefulQueryService>,
    context_service: Arc<StatefulContextService>,
}
```

构建顺序：

```text
AppConfig
→ SourceRegistry::from_config
→ Arc<SourceRegistry>
→ StatefulQueryService::new
→ StatefulContextService::from_query_service
→ LogQueryServer
```

三个工具必须共享同一个 `StatefulQueryService` / `MatchReferenceStore`，否则 `search_logs` 生成的 `match_ref` 无法被 `get_log_context` 解析。

## 16. MCP 输入输出 Rust 类型

类型必须与 `schemas/mcp-tools-v1.schema.json` 完全一致。

要求：

- `serde(deny_unknown_fields)`。
- 使用 Schemars / rmcp 官方推荐方式生成输入 Schema。
- 字段默认值与文档一致。
- 不在响应中加入未冻结字段。
- 不返回内部 summary、累计资源、偏移、inode 或调试字段。

### `list_log_sources`

请求：空对象。

调用：

```rust
registry.list()
```

响应只包含：

```text
source_id
name
description
service
environment
tags
```

不得包含：

```text
root
files
directories
relative_path
配置文件位置
```

### `search_logs`

输入：

```text
source_ids
keyword
case_sensitive = false
start_time = null
end_time = null
order = oldest_first
max_results = 50
cursor = null
```

要求：

- `order` 枚举 v1 只能为 `oldest_first`。
- 显式拒绝 `newest_first`，映射 `INVALID_ARGUMENT`。
- 使用 `StatefulQueryRequest`，不要直接调用扫描器或 `QueryEngine` 绕过 cursor / match_ref。
- `max_results` 缺省时使用冻结默认值 50；仍受服务端配置上限约束。
- cursor 原样作为不透明 token 传给 `StatefulQueryRequest::with_cursor`。
- 为每个调用创建 CancellationToken，并尽可能绑定 rmcp 请求生命周期。

输出严格为：

```text
results[]:
  match_ref
  source_id
  file_id
  file_name
  line_number
  timestamp
  content
  content_truncated
truncated
next_cursor
```

注意：

- 不输出 `content_lossy`、`original_line_bytes`、内部偏移或 summary，除非先通过 v1 契约变更；当前禁止变更。
- `timestamp` 使用 RFC 3339；未知时间为 `null`。
- 无匹配为成功空数组。
- `truncated` 使用 `StatefulQueryPage.truncated`。
- `next_cursor` 使用服务端生成值。
- `continuation_unavailable=true` 时不得伪造 cursor。

### `get_log_context`

输入：

```text
match_ref
before_lines = 0
after_lines = 0
```

要求：

- 只调用 `StatefulContextService::get_context`。
- 不接受 path、file_id、line_number 或 byte_offset。
- 使用与 query service 共享的 MatchReferenceStore。

输出严格为：

```text
source_id
file_id
file_name
start_line
end_line
lines[]:
  line_number
  content
truncated
```

不得输出内部扫描字节、is_match_line、原始行字节、before_truncated、after_truncated 等未冻结字段。

## 17. 请求取消

Codex 必须检查当前 rmcp 版本是否公开请求取消/上下文信号。

优先顺序：

1. 若 rmcp handler context 提供取消 token，桥接到 `tokio_util::sync::CancellationToken`。
2. 若只提供 Future 生命周期，使用 Drop guard，在工具 Future 被丢弃时取消 token。
3. 不得声称已完成客户端断连取消，除非有协议级测试证明。
4. 未完成的端到端断连行为必须写入 PR 的 Known limitations。

## 18. MCP 工具错误

业务错误必须：

```text
CallToolResult.isError = true
```

第一个文本内容块必须是 PR A 的紧凑 `ToolError` JSON。

不得：

- 把内部错误 Display 直接返回。
- 在成功结果里嵌入 `{error: ...}`。
- 对无匹配返回错误。
- 将 cursor 错误错误地映射为 INTERNAL_ERROR。

## 19. 成功响应大小限制

在构建 rmcp 成功结果前：

1. 构建冻结的输出对象。
2. 使用 PR A 的完整 JSON 序列化检查。
3. 上限取 `registry.limits().max_response_bytes`。
4. 超限映射为 `RESOURCE_LIMIT`。
5. 不得 post-hoc 删除结果同时保留会跳过数据的 cursor。

如果 rmcp 同时产生 text content 和 structured content，应明确检查哪一个是线上完整负载，并至少保证冻结 JSON 对象不超过服务上限；测试中记录序列化方式。

## 20. PR B 单元与集成测试

至少覆盖：

1. `list_log_sources` 不暴露绝对路径。
2. `search_logs` 搜索临时文件中的 `traceId`。
3. 搜索结果包含 `match_ref`。
4. 使用 `next_cursor` 获取下一页且不重复结果。
5. 修改 cursor 查询条件返回 `CURSOR_INVALID`。
6. `get_log_context` 使用搜索返回的 `match_ref`。
7. 上下文不接受任意路径或行号字段。
8. 未知来源返回 `UNKNOWN_SOURCE`。
9. 过期/未知 match_ref 返回 `MATCH_REF_INVALID`。
10. 文件轮转后上下文返回 `FILE_CHANGED`。
11. 无匹配返回成功空数组。
12. `newest_first` 被拒绝。
13. 成功和错误响应均符合机器 Schema。
14. 完整响应超限返回 `RESOURCE_LIMIT`。

建议提供不经过网络的工具层集成测试，直接调用 handler 或服务适配函数。

## 21. PR B 验收

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
python scripts/validate_contracts.py
```

通过后创建 PR，停止，等待合并。

---

# PR C：stdio、Streamable HTTP 与协议验收

## 22. 分支

从 PR B 合并后的最新 `main` 创建：

```text
feat/mcp-transports-validation
```

## 23. 正式入口

建议新增：

```text
src/main.rs
src/bin/log-query-mcp-stdio.rs
```

或等价结构。

### Streamable HTTP

- 正式服务入口。
- MCP endpoint：`/mcp`。
- 默认监听：`127.0.0.1:8000`。
- 环境变量：
  - `LOG_QUERY_MCP_CONFIG`：配置文件路径；若设置缺失或文件错误，启动失败。
  - `LOG_QUERY_MCP_BIND`：可选，默认 `127.0.0.1:8000`。
- 只有显式配置才允许监听 `0.0.0.0` 或内网地址。
- v1 不加入认证或 TLS。
- 支持 Ctrl-C / SIGTERM 优雅停止。

### stdio

- 只用于本机调试、Codex/Inspector 验证。
- stdout 只能输出 MCP 协议内容。
- 诊断日志写 stderr。
- 使用同一个配置加载和 `LogQueryServer` 构建路径。

## 24. 日志与启动错误

建议引入 `tracing` / `tracing-subscriber`：

- 不记录完整关键字。
- 不在 info 级别记录绝对路径。
- 启动失败可以在服务器 stderr 记录管理员需要的配置错误，但不能经过 MCP 返回给客户端。
- 记录 bind 地址、来源数量和生效资源限制摘要。

## 25. 协议级测试

至少实现自动化 smoke test。

### stdio 测试

1. 启动 stdio binary，使用临时配置和日志。
2. 发送 initialize。
3. 发送 notifications/initialized。
4. 调用 tools/list。
5. 确认只有三个工具。
6. 调用 list_log_sources。
7. 调用 search_logs。
8. 用结果中的 match_ref 调用 get_log_context。
9. 确认 stdout 没有非协议日志。

### Streamable HTTP 测试

1. 绑定临时端口。
2. initialize 并确认协议版本。
3. 获取并复用 `Mcp-Session-Id`。
4. 后续请求携带要求的协议头。
5. 解析 JSON 或 SSE 响应。
6. tools/list 只出现三个工具。
7. 完成 list → search → context 链路。
8. 完成 cursor 第二页。
9. DELETE 会话。
10. SIGTERM / graceful shutdown。

协议测试不要依赖公网服务。

## 26. MCP Inspector 人工验收说明

更新 README，提供：

```bash
cargo build --release --locked

LOG_QUERY_MCP_CONFIG=./log-query-mcp.json \
LOG_QUERY_MCP_BIND=127.0.0.1:8000 \
RUST_LOG=log_query_mcp=info \
  target/release/log-query-mcp
```

Inspector 验证顺序：

```text
list_log_sources
→ search_logs(traceId)
→ search_logs(next_cursor)
→ get_log_context(match_ref)
```

记录实际 Inspector 版本、MCP 协议版本和结果；没有执行过时不要写“已通过”。

## 27. PR C 验收

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
python scripts/validate_contracts.py
cargo build --release --locked
```

附加：

- stdio smoke test。
- Streamable HTTP smoke test。
- 启动失败测试。
- 默认只监听 loopback 的测试或配置断言。

---

# 统一安全和兼容性要求

## 28. 不可破坏的 v1 契约

Codex 不得改变：

```text
工具名：list_log_sources / search_logs / get_log_context
排序：oldest_first only
时间：[start_time, end_time)
case insensitive：ASCII folding only
无时间匹配：保守返回，timestamp=null
无匹配：成功空数组
cursor / match_ref：不透明、短期、单实例、重启失效
Linux：kernel >= 5.6
文件安全：openat2 + BENEATH + NO_SYMLINKS + NO_MAGICLINKS + NO_XDEV
```

## 29. 客户端永远不能提供

```text
服务器绝对路径
相对文件路径
目录
文件 glob
inode
设备号
任意 byte offset
file_id + 任意 line_number 组合
Shell 命令
```

## 30. 不能泄露的内部信息

成功和错误结果均不得泄露：

```text
来源 root
配置文件路径
绝对日志路径
设备号 / inode
内部字节偏移
Rust backtrace
系统调用参数
操作系统用户
服务器凭证
```

`file_name` 只能是来源内显示名或相对显示名。

## 31. 依赖和代码质量

- 保持 `#![forbid(unsafe_code)]`。
- 不降低 Clippy 严格度以绕过错误。
- 不提交临时 bootstrap/fixup workflow。
- 不使用 GitHub Actions 自动修改生产源代码。
- 不加入与当前任务无关的重构。
- 新增依赖必须说明原因，关闭不必要 feature。
- 所有依赖更新提交 `Cargo.lock`。

---

# Codex 输出模板

## 32. 每个 PR 最终回复必须包含

```text
Branch:
PR:
Scope completed:
Files changed:
Tests run:
CI status:
Security invariants checked:
Known limitations:
Next recommended PR:
```

不要只说“完成”；必须给出测试命令和实际结果。

## 33. 遇到以下情况时停止并报告

- 冻结文档和机器 Schema 冲突。
- 当前公共 API 无法满足冻结响应而必须改变契约。
- rmcp 已发布版本无法提供所需 tools-only Server 能力。
- 只能通过暴露路径或绕过 `SourceRegistry` 才能实现功能。
- 响应大小限制与现有 cursor 语义冲突且无法无损处理。
- 端到端取消无法由当前 rmcp API 传播。
- 测试需要 root/mount 权限。

停止时应创建说明性 issue 或在 PR 中列明证据，不得自行放宽安全要求。

---

# 完成交接的定义

## 34. Codex 工作完成条件

完成三个 PR 后，项目应满足：

- 三个 MCP 工具通过正式 rmcp Server 暴露。
- 成功响应严格符合 v1 Schema。
- 业务错误严格符合 v1 ToolError JSON。
- 完整序列化响应有硬上限。
- stdio 可用于本机调试。
- Streamable HTTP 可在回环地址启动。
- list → search → cursor → context 端到端测试通过。
- 不返回绝对路径。
- 不允许任意文件读取。
- Rustfmt、Clippy、全量测试、contracts、release build 全部通过。

仍可留给目标环境验收的事项：

- MCP Inspector 人工记录。
- 实际目标 AI 客户端兼容性。
- 客户端真实断连后的取消延迟。
- `RESOLVE_NO_XDEV` 的 mount/bind mount 集成测试。
- 1 GiB、10 GiB、大量小文件和并发性能基准。
- systemd/cgroup 资源限制实测。
