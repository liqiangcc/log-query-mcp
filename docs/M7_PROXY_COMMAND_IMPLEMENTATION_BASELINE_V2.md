# Log Query MCP v2 M7 ProxyCommand Implementation Baseline

> 状态：Core + functional + performance harness + release integration present / CI and live validation blocked  
> 日期：2026-08-10  
> Draft PR：#25  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> TODO：[`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)  
> Failure Matrix：[`M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md`](./M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)  
> Auth Gate：[`M7_PROXY_AUTH_GATE_V2.md`](./M7_PROXY_AUTH_GATE_V2.md)  
> Sync Gate：[`M7_PROXY_SYNC_GATE_V2.md`](./M7_PROXY_SYNC_GATE_V2.md)  
> Restart Gate：[`M7_PROXY_RESTART_GATE_V2.md`](./M7_PROXY_RESTART_GATE_V2.md)  
> Generation Gate：[`M7_PROXY_GENERATION_GATE_V2.md`](./M7_PROXY_GENERATION_GATE_V2.md)  
> Performance Gate：[`M7_PROXY_PERFORMANCE_GATE_V2.md`](./M7_PROXY_PERFORMANCE_GATE_V2.md)  
> ADR：[`adr/0012-use-proxy-command-as-ssh-stream-transport.md`](./adr/0012-use-proxy-command-as-ssh-stream-transport.md)

## 1. 当前实现范围

M7 当前已经具备：

- admin-only `SshConnectionConfig.proxy` / `type=command`；
- direct `program + args[]` spawn，无 Shell command string；
- whole-argument `{host}` / `{port}` placeholders；
- `SshStreamConnector` 分离 Direct TCP 与 ProxyCommand；
- Direct / ProxyCommand 都进入 `russh::client::connect_stream`；
- child stdin/stdout = SSH raw byte stream；
- strict Host Key Verification、Authentication、SFTP、Sync、Cache、Query 边界不变；
- fail-closed child cleanup + bounded stderr drain；
- stable ProxyCommand startup/stream/timeout classifications；
- password / private key / encrypted private key success harness；
- full / tail / from_now / incremental / truncate / rotation Sync harness；
- startup/auth/timeout/cancellation/active-crash failure harness；
- Direct + Proxy isolation、Local + Direct + Proxy mixed-query harness；
- server restart / stale-cache fail-closed / recovery harness；
- cursor / match_ref / generation consistency harness；
- Direct vs Proxy paired performance harness，包含 M6 large-file profiles、300 range reads、mixed concurrency 和 helper cleanup；
- README / INSTALL / OPERATIONS / PRODUCTION_CHECKLIST 的 ProxyCommand 与 WSL 交付契约；
- v2 release example 同时保留 Direct SSH 与 ProxyCommand；
- release package 包含 v2 machine schema 与 M7 ProxyCommand 交付文档；
- package validator 与 `rc_check.sh` 增加 ProxyCommand release-contract 非 live 检查。

## 2. 关注分离

```text
ProxyCommand = local byte-stream adapter
SSH          = protocol / authentication / host identity
SFTP         = remote read-only file access
Sync         = remote-to-local synchronization
Cache        = stable local snapshot
Query Engine = search
MCP          = AI-facing log API
```

M7 没有让 ProxyCommand 参与 remote path、credential、cache、sync 或 query 业务逻辑。Fault、Auth、Sync、Mixed Query、Restart、Generation、Performance、Release Integration 分别保持独立关注点。

## 3. Transport / Failure Baseline

稳定内部分类：

```text
ProxyCommandNotFound
ProxyCommandPermissionDenied
ProxyCommandStartFailed
ProxyCommandStreamFailed
ProxyCommandTimeout
```

边界保持：

- wrong host key → `HostKeyVerificationFailed`；
- wrong credential → `AuthenticationFailed`；
- active transport loss → SFTP failure，reader latch `Broken`；
- raw OS error / stderr 不进入 AI-facing error；
- Direct TCP 继续使用原 Direct 错误；
- ProxyCommand 不新增 MCP tool、remote exec、write、upload、delete 或 arbitrary remote path。

## 4. Auth / Sync Baseline

Auth 独立 gate：

```text
tests/m7_proxy_auth_live.rs
.github/workflows/m7-proxy-auth.yml
```

覆盖 unencrypted Ed25519 与 encrypted Ed25519 + `passphrase_secret_ref`，两者都经过 ProxyCommand → strict known_hosts → SSH public-key auth → read-only SFTP。passphrase 保持在 SecretResolver/SSH 层。

Sync 独立 gate：

```text
tests/m7_proxy_sync_live.rs
.github/workflows/m7-proxy-sync.yml
```

覆盖：

```text
full -> InitialBootstrap
incremental growth -> Appended / same generation
tail(bytes) -> Tail coverage
from_now -> history excluded, future append same generation
truncate -> RemoteTruncated new generation
same-path same-size replacement -> ContinuityMismatch new generation
```

## 5. Query / Recovery Baseline

已有独立 harness：

```text
M7 Mixed Query
M7 Proxy Restart
M7 Proxy Generation
```

证明：

- one request 可以返回 Local + Direct + Proxy 三源；
- failed Proxy 不污染后续 healthy source 查询；
- outage 时 `allow_stale_on_error=false` 不返回 stale success；
- restart 后重新同步并恢复；
- old cursor 持有 frozen candidate snapshot；
- fresh query 进入新 generation；
- existing match_ref 持有 source/file + generation pin；
- `get_log_context` 对 existing match_ref 可完全 cache-only。

## 6. Performance Regression Baseline

新增：

```text
tests/m7_proxy_performance_live.rs
.github/workflows/m7-proxy-performance.yml
docs/M7_PROXY_PERFORMANCE_GATE_V2.md
```

### Transport micro-benchmark

同一 OpenSSH fixture 下记录：

```text
5 x Direct open/read/close
5 x Proxy  open/read/close
300 x Proxy read_range on one SFTP session
2 x Direct + 2 x Proxy concurrent open/read/close
```

输出 paired transport metrics，不预设武断的绝对毫秒阈值。

### Large-file paired profiles

每个 profile 都依次运行 Direct 与 Proxy：

```text
100 MiB full + 1 MiB append
1 GiB full + 100 MiB append
10 GiB logical tail(64 MiB) + 1 MiB append
```

继续断言 M6 资源不变量：

```text
cold bootstrap remote read = cached payload + bounded continuity probe
unchanged remote read <= 64 KiB
incremental remote read <= payload + 2 x 64 KiB
cache-local scan remote bytes = 0
```

workflow 还在 transport benchmark、每个 large profile 和收尾阶段检查 `/usr/bin/nc 127.0.0.1 2235` 不得残留。

M6 historical Direct evidence 仍作为比较参考，但不是 M7 PASS threshold。

## 7. Release Integration Baseline

Release Integration 已覆盖：

```text
README.md
docs/INSTALL.md
docs/OPERATIONS.md
docs/PRODUCTION_CHECKLIST.md
docs/CONFIG_SCHEMA_V2.md
docs/RELEASE_READINESS_V2.md
examples/log-query-mcp.v2.remote.json
scripts/package_release.sh
scripts/validate_release_package.sh
scripts/rc_check.sh
```

交付契约：

- README 描述 Direct/ProxyCommand、WSL 宿主机网络模型、placeholder、Secret/host-key 边界；
- INSTALL 要求以 systemd 服务身份验证 helper，不为了 helper 整体关闭 hardening；
- OPERATIONS 记录 Proxy internal categories、bounded stderr、helper lifecycle、WSL 排障顺序；
- PRODUCTION_CHECKLIST 增加 ProxyCommand、WSL、performance、release artifact 验收项；
- CONFIG_SCHEMA_V2 已从“target pending”修正为 machine schema + Rust runtime 已实现状态；
- RELEASE_READINESS_V2 已纳入所有 M7 gates、WSL acceptance 和新 package contract；
- v2 example 同时包含 Direct connection 与 `proxy.type=command` connection；
- package 强制包含 `schemas/log-query-mcp-config-v2.schema.json` 和 M7 设计/验证文档；
- package validator 检查 Direct + Proxy example、允许的 placeholder 和 `ProxyCommandConfig` machine schema；
- `rc_check.sh` 在非 live 阶段检查同一 release contract。

Release Integration 不表示 RC 已通过。package/rc_check 仍需要当前 candidate 的真实执行证据。

## 8. 当前验证状态

GitHub Actions Billing / Spending Limit 仍是外部 blocker。

Release Integration 后当前 branch head `ede9c591b91c22e76a8db696db3fb8cc4336a5b4` 触发了新的 candidate runs。其中：

```text
Release run = 31382108976
package job = failure
steps       = null

Rust run    = 31382108916
test job    = failure
steps       = null
```

两者都没有执行任何 step，因此新 package/rc_check/Rust 代码没有真实 runner 结果。其他 M7 workflows 在同一 head 也继续以 runner-start 前失败结束。

当前必须记录：

```text
implementation present                 YES
functional harnesses                   IMPLEMENTED
performance harness                    IMPLEMENTED
release integration                    IMPLEMENTED
compile/rustfmt/clippy evidence         NO CURRENT PASS
M7 workflow execution                  BLOCKED
Direct SSH regression                  NO NEW PASS EVIDENCE
ProxyCommand live SSH                  NOT VALIDATED
Proxy key auth                         NOT VALIDATED
Proxy sync semantics                   NOT VALIDATED
mixed/restart/generation               NOT VALIDATED
M7 performance metrics                 NONE
release package / rc_check             NOT CURRENTLY VALIDATED
WSL acceptance                         NOT VALIDATED
RC ready                               NO
```

`steps=null` 既不能视为已知代码失败，也不能视为 PASS。

## 9. 下一阶段

代码、functional/performance harness 与 Release Integration 已基本闭合。剩余工作不应继续扩功能面，优先转向验收：

1. 准备真实 WSL → Windows Host helper → Remote SSH acceptance procedure/evidence template；
2. 在目标环境证明 WSL Direct path 不可达、Windows/VPN path 可达；
3. 以 `log-query-mcp` service identity 验证 helper、strict known_hosts、auth、SFTP 和三个 MCP tools；
4. 验证 helper 正常/失败/取消路径无残留进程；
5. Billing 恢复后执行当前 candidate 的 Rust / Contracts / Direct SSH / 全部 M7 / Release gates；
6. 记录 M7 paired performance metrics 和 release artifact evidence；
7. 只有全部 PASS 后再考虑 PR Ready / merge / tag / release。

## 10. 当前完成定义

```text
M7 design                         DONE
ADR                               DONE (0012)
config schema/runtime             DONE
stream abstraction                IMPLEMENTED / CI BLOCKED
ProxyCommand core connector       IMPLEMENTED / CI BLOCKED
child cleanup + stderr            IMPLEMENTED / CI BLOCKED
failure classification            IMPLEMENTED / CI BLOCKED
functional live harnesses         IMPLEMENTED / EXECUTION BLOCKED
failure matrix harness            EXPANDED / EXECUTION BLOCKED
mixed/restart/generation harness  IMPLEMENTED / EXECUTION BLOCKED
performance regression harness    IMPLEMENTED / EXECUTION BLOCKED
release docs/integration          IMPLEMENTED / VALIDATION BLOCKED
WSL acceptance                    PENDING REAL TARGET
final gates                       BLOCKED / NOT PASS
RC ready                          NO
```

真实 gates 与 WSL 目标验收通过前，不应把 M7 标记 production-ready，也不应把 PR #25 转为 Ready。
