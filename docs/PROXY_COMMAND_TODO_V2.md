# Log Query MCP v2 M7 ProxyCommand 实施 TODO

> 状态：Core + functional + performance harness + release integration + WSL acceptance tooling implemented / CI & real target validation blocked  
> 日期：2026-08-10  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> 实现基线：[`M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md`](./M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)  
> Failure Matrix：[`M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md`](./M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)  
> Restart Gate：[`M7_PROXY_RESTART_GATE_V2.md`](./M7_PROXY_RESTART_GATE_V2.md)  
> Generation Gate：[`M7_PROXY_GENERATION_GATE_V2.md`](./M7_PROXY_GENERATION_GATE_V2.md)  
> Auth Gate：[`M7_PROXY_AUTH_GATE_V2.md`](./M7_PROXY_AUTH_GATE_V2.md)  
> Sync Gate：[`M7_PROXY_SYNC_GATE_V2.md`](./M7_PROXY_SYNC_GATE_V2.md)  
> Performance Gate：[`M7_PROXY_PERFORMANCE_GATE_V2.md`](./M7_PROXY_PERFORMANCE_GATE_V2.md)  
> WSL Acceptance：[`M7_WSL_ACCEPTANCE_V2.md`](./M7_WSL_ACCEPTANCE_V2.md)  
> systemd HTTP Acceptance：[`M7_WSL_SYSTEMD_HTTP_ACCEPTANCE_V2.md`](./M7_WSL_SYSTEMD_HTTP_ACCEPTANCE_V2.md)  
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
- [x] `CONFIG_SCHEMA_V2.md` 目标契约。
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

- [x] stable ProxyCommand startup / stream / timeout classifications。
- [x] wrong host key 保留 `HostKeyVerificationFailed`。
- [x] auth failure 保留 `AuthenticationFailed`。
- [x] timeout/cancellation PID cleanup + semaphore release harness。
- [x] stderr flood >64 KiB harness。
- [x] authentication failure child cleanup harness。
- [x] active-session crash → SFTP failure → Broken latch harness。
- [x] workflow-level orphan helper assertion。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

## 5. M7-4 Functional Gates — HARNESS IMPLEMENTED / EXECUTION BLOCKED

### Success / Auth / Sync

- [x] real OpenSSH + `/usr/bin/nc {host} {port}`。
- [x] password auth / strict known_hosts / wrong-host-key / bounded SFTP read harness。
- [x] unencrypted private key auth。
- [x] encrypted private key + passphrase。
- [x] full / tail / from_now sync。
- [x] incremental append / rotation / truncate。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

独立 gates：

```text
M7 ProxyCommand
M7 Proxy Auth
M7 Proxy Sync
```

### Failure Matrix

- [x] program not found / permission denied / early exit / stdout EOF。
- [x] stderr flood + connect timeout。
- [x] cancellation / child reap / semaphore release。
- [x] wrong password → `AuthenticationFailed` + child reap。
- [x] active Proxy crash / network disconnect / SFTP Broken latch。
- [x] stalled Proxy + active Direct isolation。
- [x] workflow orphan-process assertion。

### Restart / Stale Cache

- [x] bootstrap through ProxyCommand。
- [x] sshd outage → `REMOTE_UNAVAILABLE`。
- [x] `allow_stale_on_error=false` 不返回 stale success。
- [x] 保留最后有效 generation 用于恢复。
- [x] restart + append resync / generation advance。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

### Cursor / MatchRef / Generation

- [x] old cursor 保持 frozen snapshot。
- [x] fresh query 看到新 generation。
- [x] Source A / B match_ref 不串 source/file。
- [x] Source A replacement 后 old match_ref 保持 pinned old generation。
- [x] known_hosts 不可用时 existing match_ref context 仍 cache-only 可读。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

## 6. M7-5 Mixed Transport / Query — HARNESS IMPLEMENTED / EXECUTION BLOCKED

- [x] transport-level Direct + stalled Proxy isolation。
- [x] Direct/Proxy shared global SSH semaphore。
- [x] `Local + Direct + Proxy` mixed query。
- [x] failed Proxy 显式 `REMOTE_UNAVAILABLE`。
- [x] failed Proxy 后 Local + Direct + healthy Proxy 仍可继续 query。
- [x] cursor/match_ref generation consistency through Proxy source。
- [ ] actual PASS evidence — **BLOCKED by Billing**。

## 7. WSL Acceptance — TOOLING IMPLEMENTED / REAL TARGET PENDING

交付：

- [x] `docs/M7_WSL_ACCEPTANCE_V2.md` 真实目标验收契约。
- [x] `scripts/m7_wsl_acceptance.py` stdio MCP acceptance client。
- [x] `scripts/m7_wsl_acceptance.sh` 默认强制实际 service identity。
- [x] static mode 验证 Proxy source / Windows helper / `{host}` / `{port}` contract。
- [x] real mode 验证 WSL Direct TCP gap。
- [x] real mode 执行 MCP initialize / tools/list / 三个 MCP tools。
- [x] evidence JSON 默认去除 Secret、日志正文、match_ref、明文 logical host。
- [x] Windows helper before/after process-count cleanup evidence。
- [x] `scripts/m7_wsl_http_acceptance.py` 自动验证生产 systemd Streamable HTTP Proxy source。
- [x] systemd HTTP gate 验证 `ActiveState/User/MainPID`。
- [x] systemd HTTP gate 验证 `/proc/<MainPID>/exe` 与 expected candidate binary SHA256 一致。
- [x] systemd HTTP gate 自动执行 initialize / tools-list / 三个 MCP tools，并输出独立去敏 evidence。
- [x] stdio + HTTP acceptance tooling 均接入 release package / validator / `rc_check.sh` static precheck。

真实目标仍必须执行：

- [ ] WSL `log-query-mcp` → Windows executable ProxyCommand → Remote SSH。
- [ ] Windows/VPN host path 可达，WSL Direct path 确认不可用。
- [ ] 实际 `log-query-mcp` service identity 可启动 Windows helper。
- [ ] strict known_hosts 仍绑定逻辑目标。
- [ ] stdio `list_log_sources` PASS。
- [ ] stdio `search_logs` PASS。
- [ ] stdio `get_log_context` PASS。
- [ ] stdio helper cleanup PASS。
- [ ] production systemd HTTP healthcheck PASS。
- [ ] production systemd HTTP acceptance client PASS。
- [ ] production systemd service 对同一 Proxy source 三工具 PASS。
- [ ] systemd helper cleanup PASS。
- [ ] traceable redacted stdio + HTTP evidence 保存。

> WSL 交互会话能启动 `.exe` 不代表 systemd service 一定拥有相同 Windows interop 条件。因此 Final Acceptance 必须包含实际 systemd Proxy source search，不允许只用 stdio acceptance 替代。

## 8. M7-6 Performance / Regression — HARNESS IMPLEMENTED / EXECUTION BLOCKED

独立 gate：`.github/workflows/m7-proxy-performance.yml`

- [x] Direct 5-session connection setup measurement harness。
- [x] ProxyCommand 5-session setup latency measurement harness。
- [x] 100 MiB full：Direct + Proxy paired profile。
- [x] 1 GiB full：Direct + Proxy paired profile。
- [x] 10 GiB logical tail(64 MiB)：Direct + Proxy paired profile。
- [x] unchanged continuity probe <= 64 KiB assertion。
- [x] incremental append bounded transfer assertion。
- [x] cache-local scan = 0 remote bytes assertion。
- [x] 2 Direct + 2 Proxy concurrency harness。
- [x] ProxyCommand 300 range reads regression harness。
- [x] normal-path orphan `/usr/bin/nc` assertions after benchmark phases。
- [x] metrics + `/usr/bin/time -v` + environment + disk evidence artifact wiring。
- [ ] actual performance metrics — **BLOCKED by Billing**。
- [ ] no unexplained performance/resource regression — **NO CURRENT EVIDENCE**。

`M7 Proxy Performance` candidate `8d116de693f2ee05381b429944e4f5033533c150` 的 run `31380836168` 中，job `proxy-performance` 为 `steps=null`。

## 9. M7-7 Documentation / Release — IMPLEMENTED / VALIDATION BLOCKED

- [x] README ProxyCommand usage / WSL 模型 / 安全边界。
- [x] INSTALL WSL/helper dependency、服务身份、systemd hardening 注意事项。
- [x] OPERATIONS Proxy diagnostics/error categories/helper lifecycle。
- [x] PRODUCTION_CHECKLIST ProxyCommand + WSL security/acceptance matrix。
- [x] v2 example config 同时包含 Direct + ProxyCommand。
- [x] Release package 强制包含 v2 machine Schema、示例和 M7 交付文档。
- [x] Release package 包含 stdio/service-identity/systemd HTTP WSL acceptance tooling 与文档。
- [x] `validate_release_package.sh` 验证 Direct+Proxy example、placeholder、`ProxyCommandConfig` machine schema 与两套 WSL acceptance clients。
- [x] `rc_check.sh` 增加 non-live ProxyCommand contract + 两套 WSL acceptance static precheck。
- [ ] 当前 candidate package/rc_check 实际 PASS — **BLOCKED until runner/local gate executes**。

## 10. Final Gate

- [ ] Contracts PASS。
- [ ] rustfmt PASS。
- [ ] Clippy `-D warnings` PASS。
- [ ] all Rust tests PASS。
- [ ] release build PASS。
- [ ] Direct SSH live PASS。
- [ ] M7 ProxyCommand success gate PASS。
- [ ] M7 Proxy Auth gate PASS。
- [ ] M7 Proxy Sync gate PASS。
- [ ] M7 ProxyCommand failure gate PASS。
- [ ] M7 Proxy restart/stale-cache gate PASS。
- [ ] M7 Proxy generation-consistency gate PASS。
- [ ] M7 mixed-query gate PASS。
- [ ] M7 Proxy performance gate PASS。
- [ ] WSL stdio service-identity acceptance PASS / traceable target evidence。
- [ ] WSL production systemd HTTP acceptance PASS / traceable target evidence。
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
functional live harnesses         IMPLEMENTED / EXECUTION BLOCKED
private/encrypted key harness     IMPLEMENTED / EXECUTION BLOCKED
sync-mode semantics harness       IMPLEMENTED / EXECUTION BLOCKED
failure matrix harness            EXPANDED / EXECUTION BLOCKED
restart/stale-cache harness       IMPLEMENTED / EXECUTION BLOCKED
generation-consistency harness    IMPLEMENTED / EXECUTION BLOCKED
Direct+Proxy isolation / mixed    IMPLEMENTED / EXECUTION BLOCKED
performance regression harness    IMPLEMENTED / EXECUTION BLOCKED
release integration               IMPLEMENTED / VALIDATION BLOCKED
WSL stdio acceptance tooling      IMPLEMENTED
systemd HTTP acceptance tooling   IMPLEMENTED
real WSL/systemd evidence         PENDING REAL TARGET
final gates                       BLOCKED / NOT PASS
RC ready                          NO
```

架构边界继续保持：

```text
ProxyCommand = local stream transport
SSH          = secure protocol/auth/host identity
SFTP         = remote read-only file transport
Sync         = remote-to-local synchronization
Cache        = stable local snapshot
Query Engine = search
MCP          = AI-facing log API
```