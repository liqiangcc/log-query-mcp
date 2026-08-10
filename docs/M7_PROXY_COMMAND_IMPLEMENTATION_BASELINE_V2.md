# Log Query MCP v2 M7 ProxyCommand Implementation Baseline

> 状态：Core + fault + mixed-query + restart + generation + auth + sync harness present / CI and live validation blocked  
> 日期：2026-08-10  
> Draft PR：#25  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> TODO：[`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)  
> Failure Matrix：[`M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md`](./M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)  
> Restart Gate：[`M7_PROXY_RESTART_GATE_V2.md`](./M7_PROXY_RESTART_GATE_V2.md)  
> Generation Gate：[`M7_PROXY_GENERATION_GATE_V2.md`](./M7_PROXY_GENERATION_GATE_V2.md)  
> Auth Gate：[`M7_PROXY_AUTH_GATE_V2.md`](./M7_PROXY_AUTH_GATE_V2.md)  
> Sync Gate：[`M7_PROXY_SYNC_GATE_V2.md`](./M7_PROXY_SYNC_GATE_V2.md)  
> ADR：[`adr/0012-use-proxy-command-as-ssh-stream-transport.md`](./adr/0012-use-proxy-command-as-ssh-stream-transport.md)

## 1. 当前实现范围

M7 当前已经具备：

- admin-only `SshConnectionConfig.proxy` / `type=command`。
- direct `program + args[]` spawn，无 Shell command string。
- only whole-argument `{host}` / `{port}` placeholders。
- `SshStreamConnector` 分离 Direct TCP 与 ProxyCommand。
- Direct / ProxyCommand 都进入 `russh::client::connect_stream`。
- ProxyCommand child stdin/stdout = SSH raw byte stream。
- strict Host Key Verification、Authentication、SFTP、Cache、Sync、Query 层不变。
- fail-closed child cleanup + bounded stderr drain。
- stable ProxyCommand startup/stream/timeout internal classifications。
- success live harness。
- expanded process/auth/timeout/cancellation/active-session failure harness。
- Direct + Proxy transport isolation harness。
- SourceRegistry / StatefulQueryService 的 Local + Direct + Proxy mixed-query harness。
- ProxyCommand server-restart / stale-cache fail-closed / recovery harness。
- Proxy source cursor / match_ref / generation pin consistency harness。
- ProxyCommand 无口令 private-key 与 encrypted private-key + passphrase auth harness。
- ProxyCommand full / tail / from_now / incremental / truncate / same-path rotation Sync harness。

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

M7 没有让 ProxyCommand 参与 remote path、credential、cache、sync 或 query 业务逻辑。Transport fault、Auth、Sync、mixed query、restart/stale-cache、generation consistency 分别使用独立 test/workflow。

## 3. Failure / Lifecycle Baseline

稳定内部分类：

```text
ProxyCommandNotFound
ProxyCommandPermissionDenied
ProxyCommandStartFailed
ProxyCommandStreamFailed
ProxyCommandTimeout
```

边界保持：

- wrong host key → `HostKeyVerificationFailed`。
- wrong credential → `AuthenticationFailed`。
- active transport loss → SFTP failure，reader latch `Broken`。
- raw OS error / stderr 不进入 AI-facing error。

Failure harness 已覆盖 startup、early EOF、stderr flood、timeout、cancellation、child reap、semaphore release、wrong-password、active proxy crash 和 Direct+Proxy isolation。

## 4. Key Authentication Baseline

独立：

```text
tests/m7_proxy_auth_live.rs
.github/workflows/m7-proxy-auth.yml
docs/M7_PROXY_AUTH_GATE_V2.md
```

覆盖：

```text
unencrypted Ed25519 private key
encrypted Ed25519 private key + passphrase_secret_ref
```

两者都经过：

```text
ProxyCommand → SSH handshake → strict known_hosts → public-key auth → SFTP → stat/read_range
```

passphrase 仍由现有 SecretResolver 解析，不进入 ProxyCommand argv/stdin/stderr。

`M7 Proxy Auth` candidate `9abb48c20801ffb0fce63ada609716652f37d88d` 的 run `31378855432` 中，`proxy-auth-live` 为 `steps=null`。

## 5. Sync Semantics Baseline

独立：

```text
tests/m7_proxy_sync_live.rs
.github/workflows/m7-proxy-sync.yml
docs/M7_PROXY_SYNC_GATE_V2.md
```

覆盖的 M4 不变量：

```text
full bootstrap
→ NewGeneration(InitialBootstrap)

incremental growth
→ Appended
→ same generation

tail(bytes)
→ Tail { start_offset }
→ only configured tail is queryable/cache payload

from_now
→ FromNow { start_offset = initial remote size }
→ history excluded
→ later append stays on same generation

truncate
→ NewGeneration(RemoteTruncated)

same-path same-size replacement
→ continuity fingerprint mismatch
→ NewGeneration(ContinuityMismatch)
```

`M7 Proxy Sync` candidate `9abb48c20801ffb0fce63ada609716652f37d88d` 的 run `31378855371` 中，`proxy-sync-live` 为 `steps=null`。

## 6. Mixed Query Integration Baseline

独立：

```text
tests/m7_mixed_query_live.rs
.github/workflows/m7-mixed-query.yml
```

Harness 已实现：

- one request 返回 Local + Direct + Proxy 三个 source；
- bad Proxy source 显式 `REMOTE_UNAVAILABLE`；
- bad Proxy failure 后同一个 query service 仍能查询 Local + Direct + healthy Proxy。

## 7. Restart / Stale-Cache Baseline

独立：

```text
tests/m7_proxy_restart_live.rs
.github/workflows/m7-proxy-restart.yml
docs/M7_PROXY_RESTART_GATE_V2.md
```

三阶段语义：

```text
Phase 1
ProxyCommand → SSH/SFTP → bootstrap cache → query PASS

Phase 2
stop sshd
→ on-query refresh fails
→ REMOTE_UNAVAILABLE
→ allow_stale_on_error=false prevents stale-success response
→ last valid generation remains stored locally

Phase 3
restart sshd
→ ProxyCommand reconnects
→ append detected/synchronized
→ cache advances
→ query recovers
```

## 8. Cursor / MatchRef / Generation Baseline

独立：

```text
tests/m7_proxy_generation_live.rs
.github/workflows/m7-proxy-generation.yml
docs/M7_PROXY_GENERATION_GATE_V2.md
```

验证：

```text
old cursor = frozen candidate snapshot
fresh query = refreshed generation
match_ref = source/file + pinned generation
get_context(existing ref) = cache-only
```

A/B 两个 Proxy source 用于验证 replacement 后不存在 generation drift 或 cross-source crossover。

## 9. 当前验证状态

GitHub Actions Billing / Spending Limit 仍是外部 blocker。

新增 gates 已被 GitHub 识别：

```text
M7 Proxy Auth
run 31378855432
job proxy-auth-live
result failure
steps null

M7 Proxy Sync
run 31378855371
job proxy-sync-live
result failure
steps null
```

runner 未执行任何 step。

因此当前必须记录：

```text
implementation present                 YES
failure classification                 IMPLEMENTED
success live harness                   IMPLEMENTED
private/encrypted key harness          IMPLEMENTED
sync-mode semantics harness            IMPLEMENTED
expanded failure harness               IMPLEMENTED
Direct+Proxy isolation harness         IMPLEMENTED
full mixed-query harness               IMPLEMENTED
restart/stale-cache harness            IMPLEMENTED
generation-consistency harness         IMPLEMENTED
compile/rustfmt/clippy evidence         NO CURRENT PASS
M7 workflow execution                  BLOCKED
Direct SSH regression                  NO NEW PASS EVIDENCE
ProxyCommand live SSH                  NOT VALIDATED
Proxy key auth                         NOT VALIDATED
Proxy sync semantics                   NOT VALIDATED
mixed query                            NOT VALIDATED
restart/stale-cache                    NOT VALIDATED
generation consistency                 NOT VALIDATED
WSL acceptance                         NOT VALIDATED
performance regression                 NOT VALIDATED
RC ready                               NO
```

`steps=null` 既不能视为已知代码失败，也不能视为 PASS。

## 10. 下一阶段

Transport、fault、Auth、Sync、mixed-query、restart/stale-cache、generation consistency 的 harness 已基本闭合。下一步优先：

1. 增加 ProxyCommand connection setup latency 与 Direct 对照。
2. 复用 M6 性能基线覆盖 100 MiB full / 1 GiB full / 10 GiB logical tail。
3. 验证 incremental append bounded transfer、Direct + Proxy concurrency、300 range reads。
4. 更新 README / INSTALL / OPERATIONS / PRODUCTION_CHECKLIST / v2 examples / rc_check。
5. Billing 恢复后执行所有当前 candidate gates。
6. 最后执行真实 WSL → Windows Host → Remote SSH acceptance。

## 11. 当前完成定义

```text
M7 design                         DONE
ADR                               DONE (0012)
config schema/runtime             DONE
stream abstraction                IMPLEMENTED / CI BLOCKED
ProxyCommand core connector       IMPLEMENTED / CI BLOCKED
child cleanup + stderr            IMPLEMENTED / CI BLOCKED
failure classification            IMPLEMENTED / CI BLOCKED
ProxyCommand live harness         IMPLEMENTED / EXECUTION BLOCKED
private/encrypted key harness     IMPLEMENTED / EXECUTION BLOCKED
sync-mode semantics harness       IMPLEMENTED / EXECUTION BLOCKED
failure matrix harness            EXPANDED / EXECUTION BLOCKED
Direct+Proxy transport isolation  IMPLEMENTED / EXECUTION BLOCKED
full mixed query                  IMPLEMENTED / EXECUTION BLOCKED
restart/stale-cache harness       IMPLEMENTED / EXECUTION BLOCKED
generation-consistency harness    IMPLEMENTED / EXECUTION BLOCKED
WSL acceptance                    TODO
performance regression            TODO
release docs/final gates          TODO
RC ready                          NO
```

真实 gates 通过前，不应把 M7 标记 production-ready，也不应把 PR #25 转为 Ready。
