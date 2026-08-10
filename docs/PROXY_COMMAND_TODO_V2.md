# Log Query MCP v2 M7 ProxyCommand 实施 TODO

> 状态：Core + failure classification + partial fault harness implemented / CI & live validation blocked  
> 日期：2026-08-10  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> 实现基线：[`M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md`](./M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)  
> Failure Matrix：[`M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md`](./M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)  
> ADR：[`adr/0012-use-proxy-command-as-ssh-stream-transport.md`](./adr/0012-use-proxy-command-as-ssh-stream-transport.md)  
> Draft PR：#25

## 0. 冻结边界

- AI-facing MCP 工具仍只有 `list_log_sources`、`search_logs`、`get_log_context`。
- 不新增 `ssh_exec` / `run_command` / Shell / arbitrary remote path / write / upload / deploy。
- ProxyCommand 只能来自管理员静态配置。
- ProxyCommand 只提供 SSH raw byte stream。
- SSH Authentication、strict Host Key Verification、SFTP、Sync、Cache、Snapshot、Query Engine 语义不回退。
- Direct TCP 继续作为无 `proxy` 配置时的默认 Transport。
- 失败继续 fail-closed，不静默使用 stale cache。

## 1. M7-0 Design / Contract — DONE

- [x] `PROXY_COMMAND_TRANSPORT_V2.md`。
- [x] ADR-0012。
- [x] `CONFIG_SCHEMA_V2.md`。
- [x] Machine JSON Schema。
- [x] Rust config/runtime validation。
- [x] `{host}` / `{port}` whole-argv placeholder。
- [x] 禁止 Shell command string / credential placeholder / AI dynamic proxy config。
- [x] valid/invalid contract fixtures。

## 2. M7-1 Stream Abstraction — IMPLEMENTED / DIRECT REGRESSION BLOCKED

- [x] `SshStreamConnector`。
- [x] `DirectConnector`。
- [x] Direct 迁移到 `russh::client::connect_stream`。
- [x] 保持 global SSH semaphore、KnownHostsClient、Auth/SFTP 边界。
- [ ] Direct SSH full regression PASS — **BLOCKED before runner start by Billing**。

## 3. M7-2 ProxyCommand Connector — IMPLEMENTED / CI BLOCKED

- [x] `ProxyCommandConfig`。
- [x] `tokio::process::Command`。
- [x] direct `program + argv[]` spawn。
- [x] stdin/stdout → SSH raw stream。
- [x] bounded stderr drain。
- [x] `kill_on_drop + start_kill + async wait/reap` cleanup baseline。
- [x] strict host-key/auth/SFTP remain above ProxyCommand。

## 4. M7-3 Lifecycle / Failure Classification — IMPLEMENTED / EVIDENCE BLOCKED

### Stable internal classification

- [x] `ProxyCommandNotFound`。
- [x] `ProxyCommandPermissionDenied`。
- [x] `ProxyCommandStartFailed`。
- [x] `ProxyCommandStreamFailed`。
- [x] `ProxyCommandTimeout`。
- [x] wrong host key 保留 `HostKeyVerificationFailed`。
- [x] auth failure 保留 `AuthenticationFailed`。
- [x] Direct errors 不改为 ProxyCommand errors。
- [x] 不向 AI 暴露 raw OS error / raw stderr。

### Lifecycle harness

- [x] timeout 路径 PID cleanup assertion 已实现。
- [x] cancellation 路径 PID cleanup assertion 已实现。
- [x] cancellation 后 global SSH semaphore release assertion 已实现。
- [x] stderr flood >64 KiB fixture 已实现。
- [x] workflow-level orphan helper assertion 已实现。
- [ ] 上述测试真实 PASS evidence — **BLOCKED by Billing**。

## 5. M7-4 Success Live Gate — HARNESS IMPLEMENTED / EXECUTION BLOCKED

独立 gate：`.github/workflows/m7-proxy-command.yml`

- [x] real OpenSSH fixture。
- [x] `/usr/bin/nc {host} {port}` stdio ProxyCommand。
- [x] password auth test harness。
- [x] strict known_hosts success harness。
- [x] wrong host key fail-closed harness。
- [x] SFTP bounded range-read harness。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

后续成功链路仍需扩展：

- [ ] private key auth。
- [ ] encrypted private key + passphrase。
- [ ] full/tail/from_now sync。
- [ ] incremental append / rotation / truncate。
- [ ] Remote Query through cache。

## 6. M7-4 Failure Matrix — PARTIAL HARNESS IMPLEMENTED / EXECUTION BLOCKED

独立 gate：`.github/workflows/m7-proxy-command-failures.yml`

已实现 harness：

- [x] program not found。
- [x] permission denied / non-executable program。
- [x] early exit / stdout EOF。
- [x] stderr flood bounded + connect timeout。
- [x] cancellation。
- [x] cancellation child reap。
- [x] cancellation semaphore release。
- [x] workflow final orphan-process assertion。

待补：

- [ ] wrong password through ProxyCommand。
- [ ] active ProxyCommand crash during SFTP session。
- [ ] network disconnect through ProxyCommand。
- [ ] SFTP operation failure through ProxyCommand。
- [ ] server restart through ProxyCommand。
- [ ] silent stale-cache fallback rejection evidence。

当前 `M7 ProxyCommand Failures` workflow 已被 GitHub 正确识别。候选 `efd07da701bc3e18a89c3204a797d32e01229982` 的 PR run `31372138200` job `proxy-command-failures` 为 `steps=null`，说明 runner 未启动。

## 7. M7-5 Mixed Transport / WSL Acceptance

### Mixed Transport

至少覆盖：

```text
Local Source
Direct Remote A
ProxyCommand Remote B
ProxyCommand Remote C
```

- [ ] mixed query。
- [ ] Proxy server failure isolation。
- [ ] Direct/Proxy 共用 global SSH semaphore。
- [ ] cursor/match_ref generation consistency。

### WSL Acceptance

目标：

```text
WSL direct path unavailable
Windows host path available
```

- [ ] WSL `log-query-mcp` → Windows executable ProxyCommand → Remote SSH。
- [ ] `list_log_sources` PASS。
- [ ] `search_logs` PASS。
- [ ] `get_log_context` PASS。
- [ ] direct path 确认不可用。
- [ ] child cleanup PASS。

## 8. M7-6 Performance / Regression

- [ ] Direct connection setup regression。
- [ ] ProxyCommand setup latency。
- [ ] 100 MiB full bootstrap。
- [ ] 1 GiB full bootstrap。
- [ ] 10 GiB logical tail。
- [ ] incremental append bounded transfer。
- [ ] Direct + Proxy concurrency。
- [ ] 300 range reads regression。
- [ ] no deadlock / process leak / unbounded buffering。

## 9. M7-7 Documentation / Release

- [ ] README ProxyCommand usage。
- [ ] INSTALL WSL/helper dependency。
- [ ] OPERATIONS Proxy process diagnostics/error categories。
- [ ] PRODUCTION_CHECKLIST ProxyCommand security acceptance。
- [ ] v2 example config Direct + Proxy examples。
- [ ] Release package includes latest Schema/examples/docs。
- [ ] `rc_check.sh` includes new non-live M7 checks。

## 10. Final Gate

- [ ] Contracts PASS。
- [ ] rustfmt PASS。
- [ ] Clippy `-D warnings` PASS。
- [ ] all Rust tests PASS。
- [ ] release build PASS。
- [ ] Direct SSH live PASS。
- [ ] M7 ProxyCommand success gate PASS。
- [ ] M7 ProxyCommand failure gate PASS。
- [ ] mixed Direct/Proxy PASS。
- [ ] WSL acceptance PASS / traceable target evidence。
- [ ] Performance PASS。
- [ ] Release/package/lifecycle PASS。
- [ ] no unexplained critical failure。

GitHub Actions Billing blocker 仍需解除。任何 `steps=null` 的 workflow failure 都不能视为 code failure，也不能视为 PASS。

## 11. 当前完成定义

```text
M7 design                         DONE
ADR                               DONE (0012)
config/schema/runtime             DONE
stream abstraction                IMPLEMENTED / CI BLOCKED
ProxyCommand connector            IMPLEMENTED / CI BLOCKED
child cleanup baseline            IMPLEMENTED / CI BLOCKED
bounded stderr                    IMPLEMENTED / CI BLOCKED
failure classification            IMPLEMENTED / CI BLOCKED
success live harness              IMPLEMENTED / EXECUTION BLOCKED
failure matrix harness            PARTIAL / EXECUTION BLOCKED
mixed Direct+Proxy                TODO
WSL acceptance                    TODO
performance regression            TODO
release docs/final gates          TODO
RC ready                          NO
```

架构边界继续保持：

```text
ProxyCommand = local stream transport
SSH          = secure protocol/auth/host identity
SFTP         = remote read-only file transport
Cache        = stable local snapshot
Query Engine = search
MCP          = AI-facing log API
```
