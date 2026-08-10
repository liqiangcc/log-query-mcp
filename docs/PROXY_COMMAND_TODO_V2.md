# Log Query MCP v2 M7 ProxyCommand 实施 TODO

> 状态：Config contract implemented / stream abstraction pending  
> 日期：2026-08-10  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> ADR：[`adr/0012-use-proxy-command-as-ssh-stream-transport.md`](./adr/0012-use-proxy-command-as-ssh-stream-transport.md)  
> 配置契约：[`CONFIG_SCHEMA_V2.md`](./CONFIG_SCHEMA_V2.md)  
> Draft PR：#25

## 0. 冻结边界

M7 必须继续遵守现有 v2 安全模型：

- AI-facing MCP 工具仍只有 `list_log_sources`、`search_logs`、`get_log_context`。
- 不新增 `ssh_exec` / `run_command` / Shell / arbitrary remote path / write / upload / deploy。
- ProxyCommand 只能来自管理员静态配置。
- ProxyCommand 只提供 SSH raw byte stream，不参与日志查询、远程 grep 或远程文件业务逻辑。
- SSH Authentication、strict Host Key Verification、SFTP-only、Sync、Cache、Snapshot、Query Engine 语义不回退。
- Direct TCP 继续作为无 `proxy` 配置时的默认 Transport。
- 失败继续 fail-closed，不静默使用 stale cache。

## 1. M7-0 Design / Contract — CONFIG LAYER IMPLEMENTED

- [x] 编写 `PROXY_COMMAND_TRANSPORT_V2.md`。
- [x] 新增 ADR-0012，冻结 ProxyCommand 只作为 SSH 字节流 Transport。
- [x] 更新 `CONFIG_SCHEMA_V2.md`，定义目标 `proxy.type=command` 契约。
- [x] 定义 Placeholder 首期只允许完整 argv 项 `{host}` / `{port}`。
- [x] 定义禁止 Shell command string、credential argv 注入和 AI 动态代理配置。
- [x] 定义 child-process 生命周期、stderr bound/redaction 和 fail-closed 原则。
- [ ] 同步修正 `PROXY_COMMAND_TRANSPORT_V2.md` 中历史 ADR 建议编号，从 ADR-0011 对齐为 ADR-0012。
- [x] 更新机器可读 `schemas/log-query-mcp-config-v2.schema.json`。
- [x] 更新 Rust v2 config structs/runtime validator。
- [x] 增加 valid ProxyCommand contract fixture。
- [x] 增加 unknown placeholder / unknown field invalid fixtures。
- [x] Rust config unit tests覆盖合法 ProxyCommand、未知 placeholder、未知字段。

当前机器 Schema 与 Rust config 已接受 `proxy.type=command`；实际 ProxyCommand transport 尚未实现，因此配置存在不代表已经能够通过代理建立 SSH 连接。

最新 candidate 的 Rust/Contracts workflow 仍在 runner 启动前失败，job `steps=[]`，属于现有 GitHub Actions Billing 外部阻塞；上述配置改动尚未获得 CI PASS 证据。

## 2. M7-1 Stream Abstraction

目标：先抽离“如何建立 SSH 底层 stream”，不改变 Direct 行为。

- [ ] 新增窄 `SshStreamConnector` / 等价抽象。
- [ ] 将现有 Direct TCP 连接迁移到 `DirectConnector`。
- [ ] 统一返回可供 `russh` stream-based connect 使用的 `AsyncRead + AsyncWrite` stream。
- [ ] 保持 `SshConnectionManager` semaphore、timeout、KnownHostsClient 和认证语义。
- [ ] 不把 russh Handle / generic channel 暴露给业务层。
- [ ] Direct SSH 全量 regression PASS 后才进入 ProxyCommand connector。

验收：

```text
password auth
private key auth
encrypted private key
known_hosts
SFTP stat/lstat/read_dir/read_range
timeout
disconnect
server restart
multi-server
300 range-read regression
```

## 3. M7-2 Config / ProxyCommand Connector

- [x] JSON Schema 增加可选 `SshConnectionConfig.proxy`。
- [x] Rust Config 增加 `ProxyCommandConfig`。
- [x] `proxy.type` 首期只允许 `command`。
- [x] `program` 非空并有硬长度上限。
- [x] `args` 最多 64 项，单项有硬长度上限。
- [x] Placeholder 只允许完整 argv 项 `{host}` / `{port}`。
- [x] 未知 Placeholder 启动时 fail-fast。
- [x] 不支持 `{username}` / credential / source/path / expression。
- [ ] 使用 `tokio::process::Command` 或等价 Tokio-native process API。
- [ ] 直接 `program + argv[]` spawn，不构造 Shell command string。
- [ ] child stdin/stdout 适配为 SSH raw stream。
- [ ] 使用 `russh` stream-based connect，复用现有 Handler/Auth/SFTP。

## 4. M7-3 Lifecycle / Security

### Process 生命周期

- [ ] connect timeout 覆盖 spawn + SSH stream connect/handshake。
- [ ] Query/Sync cancellation 能终止并 wait child。
- [ ] SSH handshake 失败后清理 child。
- [ ] Authentication/SFTP 初始化失败后清理 child。
- [ ] 正常 SSH close 后回收 child。
- [ ] Proxy early exit / stdout EOF 快速失败，不无限等待。
- [ ] 无 orphan process。
- [ ] 无 leaked SSH semaphore permit。

### stdout / stderr

- [ ] stdout 仅作为协议流，不进入日志系统。
- [ ] stderr 使用 bounded collector，建议上限 64 KiB。
- [ ] stderr 超限截断。
- [ ] Public Error 不返回 raw stderr。
- [ ] 日志不输出 credential、SecretResolver value、private key content。
- [ ] 日志避免输出完整敏感 argv；最多记录 program basename / connection_id / category / exit code。

### 权限边界

- [ ] MCP 请求 schema 不新增 proxy/program/args/host/port 输入。
- [ ] AI 不能动态选择或修改 ProxyCommand。
- [ ] ProxyCommand 不获得 remote root/path。
- [ ] ProxyCommand 不能绕过 SFTP-only。
- [ ] strict Host Key Verification 继续以逻辑目标 host/port 为准。
- [ ] ProxyCommand failure 不触发 silent stale-cache fallback。

## 5. M7-4 Integration / Failure Matrix

建立真实 OpenSSH + test proxy helper 的 live gate。

### Success

- [ ] ProxyCommand SSH handshake。
- [ ] Password auth。
- [ ] Private key auth。
- [ ] Encrypted private key + passphrase。
- [ ] strict known_hosts PASS。
- [ ] SFTP stat/lstat/read_dir/read_range。
- [ ] full bootstrap。
- [ ] tail bootstrap。
- [ ] from_now bootstrap。
- [ ] incremental append。
- [ ] rotation/truncate/replacement。
- [ ] Remote query through local cache。

### Failure

- [ ] program not found。
- [ ] permission denied / spawn failure。
- [ ] proxy early exit。
- [ ] proxy stdout EOF。
- [ ] proxy stderr flood bounded。
- [ ] connect timeout。
- [ ] SSH protocol/handshake failure。
- [ ] host key mismatch。
- [ ] authentication failure。
- [ ] SFTP failure。
- [ ] network disconnect。
- [ ] cancellation。
- [ ] server restart。
- [ ] proxy crash during active session。

每项必须验证：

```text
fail closed
no orphan process
no leaked SFTP handle
no leaked SSH permit
no secret leakage
```

## 6. M7-5 Multi-Server / WSL Acceptance

### Mixed Transport

至少覆盖：

```text
Local Source
Direct Remote A
ProxyCommand Remote B
ProxyCommand Remote C
```

- [ ] mixed query 正常。
- [ ] 一个 ProxyCommand server 失败不影响其他连接。
- [ ] global SSH semaphore 对 Direct/Proxy 统一生效。
- [ ] cursor/match_ref generation consistency 不受 Transport 类型影响。

### WSL Acceptance

目标环境：

```text
WSL direct path unavailable
Windows host path available
```

验证：

```text
WSL log-query-mcp
→ Windows executable ProxyCommand
→ Windows host network / VPN
→ Remote SSH
→ SFTP
→ Sync
→ Cache
→ search_logs/get_log_context
```

- [ ] `list_log_sources` PASS。
- [ ] `search_logs` PASS。
- [ ] `get_log_context` PASS。
- [ ] direct path 确认不可用，避免假验收。
- [ ] process cleanup PASS。

## 7. M7-6 Performance / Regression

ProxyCommand 修改 `src/transport/**` 后，之前的 Final Candidate 证据不能直接作为最终 RC 证据。

重新记录：

- [ ] Direct connection setup latency regression。
- [ ] ProxyCommand connection setup latency。
- [ ] 100 MiB full bootstrap。
- [ ] 1 GiB full bootstrap。
- [ ] 10 GiB logical tail bootstrap。
- [ ] incremental append transfer bounded。
- [ ] single-server concurrency。
- [ ] dual-server concurrency。
- [ ] Direct + Proxy mixed concurrency。
- [ ] 300 continuous range-read regression。

验收重点：

```text
no deadlock
no process leak
no unbounded buffering
no unexplained material regression
```

## 8. M7-7 Documentation / Release

- [ ] `README.md` 增加 ProxyCommand 使用入口。
- [ ] `INSTALL.md` 增加 WSL / helper dependency 注意事项。
- [ ] `OPERATIONS.md` 增加 Proxy process 诊断与错误分类。
- [ ] `PRODUCTION_CHECKLIST.md` 增加 ProxyCommand 安全验收。
- [ ] v2 example config 增加 Direct 和 Proxy 两种示例。
- [ ] Release package 包含最新 Schema / example / docs。
- [x] `scripts/validate_contracts.py` 自动覆盖新增 ProxyCommand valid/invalid fixtures。
- [ ] `scripts/rc_check.sh` 覆盖新增非-live contract/tests。

## 9. Final Gate

M7 实现完成后必须重新运行完整候选门禁：

- [ ] Contracts PASS。
- [ ] `cargo fmt --all -- --check` PASS。
- [ ] Clippy `-D warnings` PASS。
- [ ] all Rust tests PASS。
- [ ] release build PASS。
- [ ] Direct SSH live gate PASS。
- [ ] ProxyCommand SSH live gate PASS。
- [ ] M6/M7 security/fault gate PASS。
- [ ] WSL acceptance PASS 或有可追溯的目标环境人工证据。
- [ ] Performance regression PASS。
- [ ] Release/package/lifecycle gate PASS。
- [ ] 无 unexplained critical failure。

GitHub Actions Billing blocker 仍需解除，但即使 Billing 恢复，也必须先完成 M7 实现并对新 candidate 重跑 Final Gate。

## 10. 完成定义

```text
M7 design                         DONE
ADR                               DONE (0012)
config target contract            DONE
machine schema                    DONE
runtime config                    DONE
config contract fixtures          DONE
stream abstraction                TODO
proxy connector                   TODO
lifecycle/security                TODO
live integration                  TODO
WSL acceptance                    TODO
performance regression            TODO
release docs/gates                TODO
RC ready                          NO
```

M7 完成后，ProxyCommand 仍然必须保持以下架构边界：

```text
ProxyCommand = local stream transport
SSH          = secure protocol/auth/host identity
SFTP         = remote read-only file transport
Cache        = stable local snapshot
Query Engine = search
MCP          = AI-facing log API
```
