# Log Query MCP v2 M7 ProxyCommand 实施 TODO

> 状态：Core + expanded fault + mixed-query harness implemented / CI & live validation blocked  
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

- [x] `ProxyCommandNotFound`。
- [x] `ProxyCommandPermissionDenied`。
- [x] `ProxyCommandStartFailed`。
- [x] `ProxyCommandStreamFailed`。
- [x] `ProxyCommandTimeout`。
- [x] wrong host key 保留 `HostKeyVerificationFailed`。
- [x] auth failure 保留 `AuthenticationFailed`。
- [x] Direct errors 不改为 ProxyCommand errors。
- [x] timeout/cancellation PID cleanup harness。
- [x] cancellation semaphore release harness。
- [x] stderr flood >64 KiB harness。
- [x] authentication failure child cleanup harness。
- [x] active-session crash → SFTP failure → Broken latch harness。
- [x] workflow-level orphan helper assertion。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

## 5. M7-4 Success Live Gate — HARNESS IMPLEMENTED / EXECUTION BLOCKED

- [x] real OpenSSH fixture。
- [x] `/usr/bin/nc {host} {port}` stdio ProxyCommand。
- [x] password auth harness。
- [x] strict known_hosts success harness。
- [x] wrong host key fail-closed harness。
- [x] SFTP bounded range-read harness。
- [ ] private key auth。
- [ ] encrypted private key + passphrase。
- [ ] full/tail/from_now sync。
- [ ] incremental append / rotation / truncate。
- [x] Remote Query through cache — mixed-query harness implemented, execution blocked。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

## 6. M7-4 Failure Matrix — EXPANDED HARNESS IMPLEMENTED / EXECUTION BLOCKED

独立 gate：`.github/workflows/m7-proxy-command-failures.yml`

- [x] program not found。
- [x] permission denied。
- [x] early exit / stdout EOF。
- [x] stderr flood + connect timeout。
- [x] cancellation / child reap / semaphore release。
- [x] wrong password through ProxyCommand → `AuthenticationFailed`。
- [x] auth failure proxy child reap。
- [x] active ProxyCommand crash after SSH/SFTP establishment。
- [x] active network disconnect / SFTP failure。
- [x] reader fail-closed Broken latch。
- [x] stalled Proxy + active Direct isolation。
- [x] workflow orphan-process assertion。

仍待补：

- [ ] server restart through ProxyCommand。
- [ ] stale-cache fallback rejection at Sync/Backend layer。
- [x] Proxy remote failure 后 Local/Direct/healthy Proxy 仍可继续 query — harness implemented。

expanded failure harness 的实际执行仍被 Billing blocker 阻塞。

## 7. M7-5 Mixed Transport / WSL Acceptance

### Mixed Transport — HARNESS IMPLEMENTED / EXECUTION BLOCKED

独立 gate：`.github/workflows/m7-mixed-query.yml`

当前 harness：

```text
Local Source
Direct Remote
ProxyCommand Remote
failed ProxyCommand Remote
```

- [x] transport-level Direct + stalled Proxy isolation harness。
- [x] Direct/Proxy shared global SSH semaphore harness。
- [x] SourceRegistry / StatefulQueryService 完整 `Local + Direct + Proxy` mixed query harness。
- [x] healthy Direct 与 Proxy 都通过 Sync/Cache 后进入 Query Engine。
- [x] 一个 failed Proxy remote 显式返回 `REMOTE_UNAVAILABLE`。
- [x] failed Proxy remote 后，Local + Direct + healthy Proxy 仍可继续 query。
- [ ] cursor/match_ref generation consistency。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

`M7 Mixed Query` candidate `bea09b5eb91bdd6ab26312bc57c6c150b8f45994` 已触发 PR run `31374390180`；job `mixed-query-live` 为 `steps=null`，runner 未启动。

### WSL Acceptance

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
- [ ] OPERATIONS Proxy diagnostics/error categories。
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
- [ ] M7 mixed query gate PASS。
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
failure matrix harness            EXPANDED / EXECUTION BLOCKED
Direct+Proxy transport isolation  IMPLEMENTED / EXECUTION BLOCKED
full mixed query                  IMPLEMENTED / EXECUTION BLOCKED
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
