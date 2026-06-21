# Log Query MCP 生产发布迭代计划

> 状态：Plan PR 草案  
> 最近更新：2026-06-21  
> 当前基线：`main` 已合并 PR #14，最新提交 `732a36a`  
> 工作方式：一次一个 PR；每个 PR 从最新 `main` 创建，CI 全绿后再进入下一项

本文是项目级开发状态和后续恢复上下文的唯一计划文档。`docs/CODEX_HANDOFF.md` 保留为 Codex 执行约束和交接提示；具体发布路线、阶段状态、验收命令和生产包定义以本文为准。

## 1. 最终目标

在 `v*` Git tag 上产出可直接部署到生产 Linux 的 `x86_64-unknown-linux-gnu` 二进制包，并提供完整安装、运维、升级、回滚和验收文档。

首个生产发布版本必须具备：

- 三个冻结 v1 MCP 工具：`list_log_sources`、`search_logs`、`get_log_context`。
- Streamable HTTP 正式入口和 stdio 调试入口。
- 统一工具错误 wire format、错误映射和完整响应大小限制。
- tag release workflow 产出 `tar.gz`、`SHA256SUMS` 和 `BUILDINFO`。
- systemd 安装包和可执行的生产运维文档。

当前已知远程状态：

- `main` 最新提交：`732a36a`。
- 无 open issue。
- open PR #1 是 spike 预研参考，不作为实现基线。

## 2. 迭代路线

| 阶段 | 分支 | 目标 | 状态 | 合并条件 |
|---|---|---|---|---|
| Plan | `docs/iteration-plan` | 新增本文档，README 增加入口链接 | In Progress | 文档描述完整路线、状态、发布目标 |
| PR A | `feat/mcp-error-response-boundary` | 工具错误 wire format、错误映射、完整响应大小限制 | Todo | Rust/Contracts CI 全绿 |
| PR B | `feat/mcp-server-tools` | rmcp Server 与三个 MCP 工具 | Todo | 工具层集成测试和 schema 验证通过 |
| PR C | `feat/mcp-transports-validation` | stdio、Streamable HTTP、协议级 smoke test | Todo | release build、stdio/http smoke 通过 |
| PR D | `feat/production-release-package` | CI 发布 tar.gz + systemd 生产包 | Todo | tag release workflow 可生成 GitHub Release artifact |
| PR E | `docs/production-operations` | 生产安装、运维、升级、回滚、验收文档 | Todo | 文档可按步骤完成部署和验证 |

状态取值：

- `Todo`：尚未开始。
- `In Progress`：当前正在实现或评审。
- `Merged`：对应 PR 已合并到 `main`。
- `Blocked`：存在明确阻塞，需要记录阻塞原因和下一步。

每个后续 PR 合并时必须更新本表状态和相关细节，避免上下文丢失。

## 3. Plan PR 范围

分支：`docs/iteration-plan`

范围：

- 新增 `docs/ITERATION_PLAN.md`。
- README 的“正式化文档”增加“生产发布迭代计划”入口。

验收命令：

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
python scripts/validate_contracts.py
```

## 4. PR A：错误边界与完整响应大小限制

分支：`feat/mcp-error-response-boundary`

目标：

- 新增 `src/tool_error.rs` 和 `src/response_limit.rs`。
- 实现 `ToolErrorCode`、`ToolError`、紧凑 JSON 输出和 `serialize_with_limit<T: Serialize>()`。
- 映射 `StatefulQueryError`、`StatefulContextError`、`QueryStateError`、`SourceRegistryError`、`ContextReadError` 到冻结 v1 错误码。
- 错误 JSON 不允许 `details`、`path`、`cause`、`backtrace` 等额外字段。

稳定错误码：

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

验收重点：

- 错误对象严格符合 `schemas/tool-error-v1.schema.json`。
- 错误消息简短英文且去敏，不直接暴露底层 `Display`。
- 序列化失败、响应超限和内部不变量破坏映射到合适错误码。
- 响应大小限制覆盖成功响应和业务错误响应。

## 5. PR B：MCP Server 与三个工具

分支：`feat/mcp-server-tools`

目标：

- 使用已发布稳定 `rmcp = 1.7.0`，开启 `server`、`macros`、`schemars` 和所需 transport feature。
- 新增 `src/mcp_model.rs`、`src/mcp_server.rs`。
- 暴露 `list_log_sources`、`search_logs`、`get_log_context`。
- MCP 层只做薄适配，不重新实现扫描、cursor 或 context。
- 成功响应严格匹配 `schemas/mcp-tools-v1.schema.json`；业务错误使用 PR A 的 `ToolError` JSON。

验收重点：

- 三个工具的输入拒绝未知字段。
- `search_logs` 不支持 `newest_first`，仍只接受 `oldest_first`。
- 不接受客户端路径、目录、glob、任意行号或字节偏移。
- 不暴露绝对路径、inode、offset、配置路径或底层系统调用文本。
- 工具层集成测试和 schema 验证通过。

## 6. PR C：传输入口与协议级验证

分支：`feat/mcp-transports-validation`

目标：

- 新增正式 HTTP binary `log-query-mcp`。
- 新增调试 binary `log-query-mcp-stdio`。
- `LOG_QUERY_MCP_CONFIG` 必填。
- `LOG_QUERY_MCP_BIND` 默认 `127.0.0.1:8000`。
- HTTP endpoint 固定 `/mcp`。
- stdout 只输出 MCP stdio 协议；诊断日志写 stderr。
- 增加 stdio 和 Streamable HTTP smoke tests。

验收重点：

- `cargo build --release --locked --bins` 通过。
- stdio smoke test 通过。
- Streamable HTTP smoke test 通过。
- 非 loopback 监听只允许显式配置。

## 7. PR D：生产发布包

分支：`feat/production-release-package`

目标：

- 新增 release packaging 脚本。
- 新增 `.github/workflows/release.yml`。
- tag `v*` 触发正式发布；`main` 和 PR 只做验证和 dry-run。
- GitHub Release 上传 `log-query-mcp-v{version}-x86_64-unknown-linux-gnu.tar.gz` 和 `SHA256SUMS`。
- tag 版本必须与 `Cargo.toml` 的 `package.version` 一致。

release workflow 必须执行：

```text
checkout tag
校验 tag 与 Cargo.toml version
构建 release binaries
运行 smoke tests
组装 tar.gz
生成 SHA256SUMS 和 BUILDINFO
上传 GitHub Release artifacts
```

## 8. PR E：生产运维文档

分支：`docs/production-operations`

目标：

- 新增 `docs/INSTALL.md`。
- 新增 `docs/OPERATIONS.md`。
- 新增 `docs/PRODUCTION_CHECKLIST.md`。
- README 改为生产入口：快速安装、配置、启动、验证、文档索引。
- 记录 MCP Inspector 和实际 AI 客户端验收步骤；未实际执行的项目标注为待验收。

验收重点：

- 文档可按步骤完成安装、启动、升级、回滚和卸载。
- 运维文档包含日志、systemd、健康检查、备份配置、权限和故障排查。
- 生产验收清单区分“已自动验证”和“需要目标 Linux 服务器人工验收”。

## 9. 发布包定义

包名：

```text
log-query-mcp-v{version}-x86_64-unknown-linux-gnu.tar.gz
```

包内容：

```text
bin/log-query-mcp
bin/log-query-mcp-stdio
examples/log-query-mcp.v1.json
systemd/log-query-mcp.service
scripts/install.sh
scripts/uninstall.sh
docs/INSTALL.md
docs/OPERATIONS.md
docs/PRODUCTION_CHECKLIST.md
BUILDINFO
SHA256SUMS
```

默认安装路径：

| 类型 | 路径 |
|---|---|
| binary | `/opt/log-query-mcp/bin` |
| config | `/etc/log-query-mcp/config.json` |
| systemd unit | `/etc/systemd/system/log-query-mcp.service` |
| service user | `log-query-mcp` |

默认监听：

```text
127.0.0.1:8000
```

非 loopback 监听只允许显式配置。v1 不内置认证和 TLS；生产暴露到非 loopback 时由内网 ACL、反向代理或上层网关负责。

## 10. CI 和测试门禁

所有 PR 必跑：

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
python scripts/validate_contracts.py
```

PR C 起增加：

```bash
cargo build --release --locked --bins
```

并增加：

- stdio smoke test。
- Streamable HTTP smoke test。

PR D release workflow 增加：

- tag 与 `Cargo.toml` version 一致性校验。
- release binaries 构建。
- smoke tests。
- `tar.gz` 组装。
- `SHA256SUMS` 和 `BUILDINFO` 生成。
- GitHub Release artifacts 上传。

## 11. 人工验收项

在目标生产 Linux 或等价环境完成：

- systemd 安装、启动、停止、重启和开机自启验证。
- `LOG_QUERY_MCP_CONFIG` 配置读取和错误提示验证。
- 默认 loopback 监听验证。
- MCP Inspector 连接 `/mcp` 并调用三个工具。
- 实际 AI 客户端连接和只读日志查询验证。
- 日志轮转、文件替换、权限变化和来源临时不可用场景验证。
- 升级和回滚流程演练。

未实际执行的验收项必须在对应 PR 描述或生产清单中标注为待验收。

## 12. 约束和假设

- 首批生产目标只支持 `x86_64-unknown-linux-gnu` glibc 动态链接二进制。
- 首批发布格式只做 `tar.gz + systemd`，不做 `.deb`、RPM 或 OCI image。
- 正式发布只由 `v*` Git tag 触发。
- v1 不内置认证和 TLS。
- 不改变冻结 v1 契约：
  - 不支持 `newest_first`。
  - 不接受客户端路径。
  - 不暴露绝对路径、inode 或 offset。
  - 不执行 shell。
  - 不引入 `unsafe`。
- MCP 层必须复用当前核心服务：
  - `AppConfig::load(...)`
  - `SourceRegistry::from_config(...)`
  - `StatefulQueryService::new(...)`
  - `StatefulQueryService::search(...)`
  - `StatefulContextService::from_query_service(...)`
  - `StatefulContextService::get_context(...)`

## 13. 状态更新规则

每个 PR 开始时：

- 确认从最新 `main` 创建分支。
- 将对应阶段状态从 `Todo` 改为 `In Progress`。
- 如发现基线变化，更新本文“当前基线”或在阶段说明中记录。

每个 PR 合并后：

- 将对应阶段状态改为 `Merged`。
- 记录合并 PR 编号和 merge commit。
- 如有新增验收命令、人工验收项或范围调整，更新本文。

如果遇到阻塞：

- 将对应阶段状态改为 `Blocked`。
- 写明阻塞原因、已尝试方案和恢复条件。
