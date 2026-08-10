# Log Query MCP v2 M7 ProxyCommand 实施 TODO

> 状态：Core + fault + mixed-query + restart + generation harness implemented / CI & live validation blocked  
> 日期：2026-08-10  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> 实现基线：[`M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md`](./M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)  
> Failure Matrix：[`M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md`](./M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)  
> Restart Gate：[`M7_PROXY_RESTART_GATE_V2.md`](./M7_PROXY_RESTART_GATE_V2.md)  
> Generation Gate：[`M7_PROXY_GENERATION_GATE_V2.md`](./M7_PROXY_GENERATION_GATE_V2.md)  
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

- [x] `ProxyCommandNotFound` / `ProxyCommandPermissionDenied` / `ProxyCommandStartFailed`。
- [x] `ProxyCommandStreamFailed` / `ProxyCommandTimeout`。
- [x] wrong host key 保留 `HostKeyVerificationFailed`。
- [x] auth failure 保留 `AuthenticationFailed`。
- [x] timeout/cancellation PID cleanup + semaphore release harness。
- [x] stderr flood >64 KiB harness。
- [x] authentication failure child cleanup harness。
- [x] active-session crash → SFTP failure → Broken latch harness。
- [x] workflow-level orphan helper assertion。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

## 5. M7-4 Success / Failure / Restart / Generation Gates

### Success Live Gate

- [x] real OpenSSH + `/usr/bin/nc {host} {port}`。
- [x] password auth / strict known_hosts / wrong-host-key / bounded SFTP read harness。
- [x] Remote Query through Cache — mixed-query harness implemented。
- [ ] private key auth。
- [ ] encrypted private key + passphrase。
- [ ] full/tail/from_now sync。
- [ ] incremental append / rotation / truncate。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

### Failure Matrix

- [x] program not found / permission denied / early exit / stdout EOF。
- [x] stderr flood + connect timeout。
- [x] cancellation / child reap / semaphore release。
- [x] wrong password → `AuthenticationFailed` + child reap。
- [x] active Proxy crash / network disconnect / SFTP Broken latch。
- [x] stalled Proxy + active Direct isolation。
- [x] workflow orphan-process assertion。

### Restart / Stale-Cache Gate — HARNESS IMPLEMENTED / EXECUTION BLOCKED

独立 gate：`.github/workflows/m7-proxy-restart.yml`

- [x] Phase 1：通过 ProxyCommand bootstrap cache。
- [x] Phase 2：停止 sshd 后 on-query refresh 显式 `REMOTE_UNAVAILABLE`。
- [x] Phase 2：`allow_stale_on_error=false` 不把最后有效 cache 作为成功查询结果返回。
- [x] Phase 2：最后有效 cache generation 保留用于恢复。
- [x] Phase 3：重启 sshd 后 ProxyCommand reconnect。
- [x] Phase 3：追加内容重新同步并推进 cache generation。
- [ ] restart/stale-cache gate actual PASS — **BLOCKED by Billing**。

`M7 Proxy Restart` candidate `c0f9b819dd94190397dde9cd60e89a19ddd7cd50` 的 PR run `31374965163` 中，job `proxy-restart-live` 为 `steps=null`。

### Cursor / MatchRef Generation Gate — HARNESS IMPLEMENTED / EXECUTION BLOCKED

独立 gate：`.github/workflows/m7-proxy-generation.yml`

- [x] Proxy source A 首页产生 cursor。
- [x] cursor 创建后远端 append。
- [x] 旧 cursor 第二页仍读取首次 query 的 frozen snapshot，不看到 append 后内容。
- [x] fresh query 看到 append 后的新 generation。
- [x] Source A / Source B 分别生成独立 match_ref。
- [x] Source A replacement 后 fresh query 进入 replacement generation。
- [x] existing Source A match_ref 仍读取 replacement 前 pinned generation。
- [x] Source B match_ref 保持 Source B source/file/generation，不串到 Source A。
- [x] 暂时移走 known_hosts 后 existing match_ref context 仍 cache-only 可读。
- [ ] generation-consistency gate actual PASS — **BLOCKED by Billing**。

`M7 Proxy Generation` candidate `90c45a56820774208f42c6c198deda253c3016d9` 的 PR run `31378377040` 中，job `proxy-generation-live` 为 `steps=null`。

## 6. M7-5 Mixed Transport / Query — HARNESS IMPLEMENTED / EXECUTION BLOCKED

独立 gate：`.github/workflows/m7-mixed-query.yml`

当前 harness：

```text
Local Source
Direct Remote
ProxyCommand Remote
failed ProxyCommand Remote
```

- [x] transport-level Direct + stalled Proxy isolation。
- [x] Direct/Proxy shared global SSH semaphore。
- [x] `Local + Direct + Proxy` SourceRegistry / StatefulQueryService mixed query。
- [x] 一个 failed Proxy remote 显式 `REMOTE_UNAVAILABLE`。
- [x] failed Proxy 后 Local + Direct + healthy Proxy 仍可继续 query。
- [x] cursor/match_ref generation consistency through Proxy source — dedicated harness implemented。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

## 7. WSL Acceptance

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
- [ ] M7 Proxy restart/stale-cache gate PASS。
- [ ] M7 Proxy generation-consistency gate PASS。
- [ ] M7 mixed-query gate PASS。
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
child cleanup + stderr            IMPLEMENTED / CI BLOCKED
failure classification            IMPLEMENTED / CI BLOCKED
success live harness              IMPLEMENTED / EXECUTION BLOCKED
failure matrix harness            EXPANDED / EXECUTION BLOCKED
restart/stale-cache harness       IMPLEMENTED / EXECUTION BLOCKED
generation-consistency harness    IMPLEMENTED / EXECUTION BLOCKED
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
