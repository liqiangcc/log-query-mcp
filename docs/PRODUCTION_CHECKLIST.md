# Log Query MCP 生产验收清单

本文用于首个生产发布和每次升级前后的验收。没有在目标环境实际执行的项目必须保持为“待验收”。

## 1. 自动验证项

这些项目由 CI 或本地发布验证覆盖，不等同于目标生产服务器验收。

| 项目 | 状态 | 证据 |
|---|---|---|
| Rust 格式检查 | 已自动验证 | `cargo fmt --all -- --check` |
| Clippy 严格检查 | 已自动验证 | `cargo clippy --locked --all-targets --all-features -- -D warnings` |
| 全量测试 | 已自动验证 | `cargo test --locked --all-targets --all-features` |
| v1 contract/schema 校验 | 已自动验证 | `python3 scripts/validate_contracts.py` |
| release binaries 构建 | 已自动验证 | `cargo build --release --locked --bins --target x86_64-unknown-linux-gnu` |
| stdio smoke test | 已自动验证 | `tests/mcp_transport_smoke.rs` |
| Streamable HTTP smoke test | 已自动验证 | `tests/mcp_transport_smoke.rs` |
| release package dry-run | 已自动验证 | `scripts/package_release.sh --target x86_64-unknown-linux-gnu --out-dir dist --require-docs` |
| tag 与版本一致性校验 | 已自动验证 | `scripts/check_release_tag.sh v{version}` |

## 2. 发布包验收

| 项目 | 状态 | 记录 |
|---|---|---|
| GitHub Release tag 为 `v{Cargo.toml package.version}` | 待验收 |  |
| 下载 `tar.gz` 和 `SHA256SUMS` | 待验收 |  |
| `sha256sum -c SHA256SUMS` 通过 | 待验收 |  |
| 包内 `sha256sum -c SHA256SUMS` 通过 | 待验收 |  |
| `BUILDINFO` 记录版本、target、commit、构建时间和 rustc | 待验收 |  |
| 包内容包含三份生产文档 | 待验收 |  |

## 3. 目标服务器安装验收

| 项目 | 状态 | 记录 |
|---|---|---|
| Linux kernel `>= 5.6` | 待验收 |  |
| 服务器为 `x86_64` glibc 环境 | 待验收 |  |
| systemd 可用 | 待验收 |  |
| `scripts/install.sh` 以 root 成功执行 | 待验收 |  |
| 创建或复用 `log-query-mcp` 用户和组 | 待验收 |  |
| 二进制安装到 `/opt/log-query-mcp/bin` | 待验收 |  |
| 配置文件安装到 `/etc/log-query-mcp/config.json` | 待验收 |  |
| systemd unit 安装到 `/etc/systemd/system/log-query-mcp.service` | 待验收 |  |
| 配置文件权限为 root 可写、服务组可读 | 待验收 |  |

## 4. 配置和权限验收

| 项目 | 状态 | 记录 |
|---|---|---|
| 所有 `source_id` 已审批 | 待验收 |  |
| `root` 都是绝对路径且不是符号链接 | 待验收 |  |
| 未配置过宽目录，如 `/` 或整个 `/var` | 待验收 |  |
| `files` 或 `directories` 与实际轮转策略一致 | 待验收 |  |
| `log-query-mcp` 用户可读取白名单日志 | 待验收 |  |
| 非白名单日志不可读取或不可通过工具访问 | 待验收 |  |
| limits 与目标日志规模匹配 | 待验收 |  |

## 5. 服务启动验收

| 项目 | 状态 | 记录 |
|---|---|---|
| `systemctl enable --now log-query-mcp.service` 成功 | 待验收 |  |
| `systemctl status` 显示 active | 待验收 |  |
| `journalctl` 无配置错误、权限错误或 panic | 待验收 |  |
| 默认监听 `127.0.0.1:8000` | 待验收 |  |
| 未经显式配置不监听非 loopback 地址 | 待验收 |  |
| 重启服务后 cursor 和 `match_ref` 失效行为符合预期 | 待验收 |  |

## 6. MCP 协议验收

| 项目 | 状态 | 记录 |
|---|---|---|
| HTTP `/mcp` initialize 请求成功 | 待验收 |  |
| MCP Inspector 可连接 Streamable HTTP URL | 待验收 |  |
| Inspector Tools 页显示三个工具 | 待验收 |  |
| `list_log_sources` 只返回批准来源 | 待验收 |  |
| `search_logs` 可按已知 trace ID 或 request ID 返回结果 | 待验收 |  |
| `get_log_context` 可用 `match_ref` 返回有限上下文 | 待验收 |  |
| 错误响应只包含 `code`、`message`、`retryable` | 待验收 |  |
| 响应不暴露绝对路径、inode、offset 或 backtrace | 待验收 |  |

MCP Inspector 记录：

```text
Inspector version:
执行人:
执行时间:
连接 URL:
工具调用样例:
结果摘要:
```

## 7. 实际 AI 客户端验收

| 项目 | 状态 | 记录 |
|---|---|---|
| AI 客户端配置 `type=streamable-http` | 待验收 |  |
| AI 客户端连接 `http://127.0.0.1:8000/mcp` 或受控网关 URL | 待验收 |  |
| AI 客户端可列出工具 | 待验收 |  |
| AI 客户端可执行真实日志搜索 | 待验收 |  |
| AI 客户端可基于 `match_ref` 读取上下文 | 待验收 |  |
| AI 客户端无法请求任意服务器路径 | 待验收 |  |
| AI 客户端查询结果满足脱敏和最小暴露要求 | 待验收 |  |

客户端记录：

```text
客户端名称:
客户端版本:
运行位置:
连接方式:
测试问题:
工具调用摘要:
结论:
```

## 8. 运维场景验收

| 项目 | 状态 | 记录 |
|---|---|---|
| 配置变更后重启生效 | 待验收 |  |
| 日志轮转后可按配置读取新文件 | 待验收 |  |
| 文件替换场景返回稳定错误或重试后恢复 | 待验收 |  |
| 权限移除后返回来源不可用而不是泄露底层路径 | 待验收 |  |
| 查询超限返回 `RESOURCE_LIMIT` | 待验收 |  |
| 服务停止和启动不影响系统其他服务 | 待验收 |  |
| 升级流程已演练 | 待验收 |  |
| 回滚流程已演练 | 待验收 |  |
| 卸载流程已演练或确认不在本次范围 | 待验收 |  |

## 9. 非 loopback 暴露验收

v1 不内置认证和 TLS。只有确有需要时才允许非 loopback 暴露。

| 项目 | 状态 | 记录 |
|---|---|---|
| 已记录暴露原因和审批 | 待验收或不适用 |  |
| 使用反向代理、上层网关或内网 ACL | 待验收或不适用 |  |
| TLS 由上游终止或专用内网链路保护 | 待验收或不适用 |  |
| 访问控制限制到批准 AI 客户端 | 待验收或不适用 |  |
| Inspector 代理没有暴露到不可信网络 | 待验收或不适用 |  |

## 10. 发布签署

```text
版本:
发布包 SHA256:
目标服务器:
配置版本或审批单:
自动验证结果:
人工验收结论:
遗留风险:
回滚方案:
验收人:
验收时间:
```
