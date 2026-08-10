# Log Query MCP v2 M7 ProxyCommand Implementation Baseline

> 状态：Core + functional + performance harness present / CI and live validation blocked  
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
- Direct vs Proxy paired performance harness，包含 M6 large-file profiles、300 range reads、mixed concurrency 和 helper cleanup。

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

M7 没有让 ProxyCommand 参与 remote path、credential、cache、sync 或 query 业务逻辑。Fault、Auth、Sync、Mixed Query、Restart、Generation、Performance 分别使用独立 test/workflow。

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

## 7. 当前验证状态

GitHub Actions Billing / Spending Limit 仍是外部 blocker。

最新 Performance harness 已被 GitHub 识别：

```text
workflow = M7 Proxy Performance
run      = 31380836168
head     = 8d116de693f2ee05381b429944e4f5033533c150
job      = proxy-performance
result   = failure
steps    = null
```

runner 没有执行任何 step，因此没有 M7 性能数字。

其他 M7 gates 也仍缺当前候选的真实 PASS evidence。当前必须记录：

```text
implementation present                 YES
functional harnesses                   IMPLEMENTED
performance harness                    IMPLEMENTED
compile/rustfmt/clippy evidence         NO CURRENT PASS
M7 workflow execution                  BLOCKED
Direct SSH regression                  NO NEW PASS EVIDENCE
ProxyCommand live SSH                  NOT VALIDATED
Proxy key auth                         NOT VALIDATED
Proxy sync semantics                   NOT VALIDATED
mixed/restart/generation               NOT VALIDATED
M7 performance metrics                 NONE
WSL acceptance                         NOT VALIDATED
RC ready                               NO
```

`steps=null` 既不能视为已知代码失败，也不能视为 PASS。

## 8. 下一阶段

核心代码与功能/性能 harness 已基本闭合。下一步优先转向 release integration：

1. README 增加 ProxyCommand / WSL 场景与安全边界；
2. INSTALL 增加 WSL → Windows helper 依赖和部署方式；
3. OPERATIONS 增加 ProxyCommand diagnostics/error categories；
4. PRODUCTION_CHECKLIST 增加 ProxyCommand security/lifecycle/performance acceptance；
5. v2 example config 增加 Direct + Proxy example；
6. `rc_check.sh` 纳入 M7 非 live contract/static checks；
7. Billing 恢复后执行当前 candidate 所有 gates；
8. 最后执行真实 WSL → Windows Host → Remote SSH acceptance。

## 9. 当前完成定义

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
release docs/integration          TODO
WSL acceptance                    TODO
final gates                       TODO
RC ready                          NO
```

真实 gates 通过前，不应把 M7 标记 production-ready，也不应把 PR #25 转为 Ready。
