# Log Query MCP v2 M6 安全与故障证据矩阵

> 状态：M6-A / M6-B / M6-C complete  
> 日期：2026-08-07  
> 代码基线：`5fb75f29c4148b29d0100e75a90174318709c1c9`  
> Rust Gate：run `31190418118` — PASS  
> Contracts Gate：run `31190417990` — PASS  
> SSH/SFTP Live Gate：run `31190417993` — PASS  
> 总计划：[`REMOTE_SSH_CACHE_TODO_V2.md`](./REMOTE_SSH_CACHE_TODO_V2.md)

## 1. 结论

M6 前半段采用“审计现有证据，只补真实缺口”的方式完成，没有复制 M2～M5 已经存在的故障测试。

当前已经证明：

```text
AI-facing MCP input
  cannot choose SSH host / credential / arbitrary remote path

Remote Transport
  read-only + host-key verified + bounded + no exec

Remote Source
  regular-file-only + symlink fail-closed

Sync / Cache
  bounded + generation-safe + crash-recoverable + quota-aware

Query
  snapshot-stable + incomplete-cache fail-closed

Multi-server
  one MCP → Server A + Server B + Local Source
```

M6 剩余工作主要是：

```text
performance evidence
production operations docs
final acceptance baseline
```

---

## 2. AI / MCP 输入面

| 风险 | 状态 | 证据 |
|---|---|---|
| 客户端提交 SSH host | PASS | `tests/m6_security.rs` unexpected-field rejection |
| 客户端提交 SSH port | PASS | `tests/m6_security.rs` |
| 客户端提交 username/password | PASS | `tests/m6_security.rs` |
| 客户端提交 `secret_ref` | PASS | `tests/m6_security.rs` |
| 客户端覆盖 connection | PASS | MCP request schema 无 connection 字段 |
| 客户端提交远程绝对路径 | PASS | MCP schema + v2 config validation |
| 客户端提交任意 remote path | PASS | Remote path 仅由管理员 source config 推导 |
| 客户端通过 `..` / `.` / empty component 逃逸 | PASS | config + `RemoteSyncTarget` validation |
| 客户端提交反斜杠 / 控制字符路径 | PASS | config + sync target validation |
| 客户端调用 shell / exec | PASS | MCP surface + exported transport API static test |
| 客户端上传 / 修改 / 删除远程文件 | PASS | read-only transport surface test |

关键不变量：

```text
MCP request
  ≠ SSH connection config
  ≠ remote filesystem API
```

---

## 3. 信息泄漏

| 风险 | 状态 | 证据 |
|---|---|---|
| `list_log_sources` 泄漏 host | PASS | `tests/m6_remote_security.rs` |
| 泄漏 username / connection_id | PASS | `tests/m6_remote_security.rs` |
| 泄漏 `secret_ref` | PASS | `tests/m6_remote_security.rs` |
| 泄漏 remote absolute root | PASS | `tests/m6_remote_security.rs` |
| 泄漏 cache absolute path | PASS | M6 security + cache fault tests |
| ToolError 泄漏 transport nested details | PASS | `tests/m6_remote_security.rs` |
| Cache corruption error 泄漏 internal IDs/path | PASS | `tests/m6_cache_faults.rs` |
| Debug 输出泄漏 remote path | PASS | Remote target / transport redaction design |
| backtrace 进入 public tool error | PASS | public error serialization assertions |

---

## 4. SSH Transport 故障矩阵

以下能力已在 M2 real OpenSSH Gate 中存在，M6 不重复实现。

| 场景 | 状态 | 预期 |
|---|---|---|
| 正确 Password | PASS | read-only SFTP 成功 |
| 错误 Password | PASS | auth failed，secret 不泄漏 |
| encrypted private key | PASS | 成功 |
| wrong private key | PASS | auth failed |
| missing known_hosts | PASS | fail-closed |
| changed host key | PASS | host-key verification failed |
| permission denied | PASS | redacted transport error |
| missing remote file | PASS | redacted error |
| large file offset range read | PASS | bounded read |
| operation timeout | PASS | reader marked broken |
| network disconnect during read | PASS | reader marked broken |
| connect cancellation | PASS | global permit released |

真实测试：`tests/ssh_transport_live.rs`。

---

## 5. Remote 文件边界

| 场景 | 状态 | 证据 |
|---|---|---|
| explicit regular file | PASS | M5 live |
| explicit symlink | PASS | M6 real SFTP symlink Gate |
| directory-discovered symlink | PASS | lstat + regular-file-only，skip |
| symlink 指向 `/etc/passwd` | PASS | 无结果、无越界读取 |
| recursive directory | PASS | v2 MVP config reject |
| suffix filter | PASS | M5 live |
| stable directory ordering | PASS | Remote backend tests |

真实测试：`tests/m6_remote_security_live.rs`。

---

## 6. SyncEngine 故障矩阵

以下主要由 M4 覆盖。

| 场景 | 状态 | 预期 |
|---|---|---|
| Full bootstrap | PASS | 新 generation |
| Tail bootstrap | PASS | bounded partial coverage |
| FromNow bootstrap | PASS | future-only coverage |
| unchanged file | PASS | bounded continuity probe，不重复完整下载 |
| append | PASS | 只同步新增 payload + fingerprint probe |
| remote truncate | PASS | new generation |
| same-size remote replacement | PASS | fingerprint mismatch → new generation |
| rapid truncate/regrow | PASS | 不错误 append |
| read failure during append | PASS | 旧 generation 保持有效 |
| sync byte budget exceeded | PASS | 不 publish partial generation |
| invalid target path | PASS | fail-closed |
| non-regular remote file | PASS | reject |

真实 SFTP sync：`tests/m4_sync_live.rs`。

---

## 7. CacheStore / Recovery

| 场景 | 状态 | 预期 |
|---|---|---|
| cache dir mode | PASS | 0700 |
| data/manifest file mode | PASS | 0600 |
| opaque cache layout | PASS | 不使用 remote path 作为本地目录名 |
| manifest validation | PASS | corruption rejected |
| orphan staging | PASS | restart cleanup/recovery |
| interrupted append staging | PASS | committed generation 不被污染 |
| global quota | PASS | bounded |
| per-source quota | PASS | bounded |
| pin-aware GC | PASS | active generation 不被回收 |
| protected current generation 无法安全驱逐 | PASS | `CACHE_LIMIT_EXCEEDED` |
| generation data shorter than manifest | PASS | fail-closed |
| generation data longer than committed length | PASS | truncate to committed length |
| active generation 被外部删除 | PASS | `CACHE_CORRUPTED` / restart reject |
| active generation 被外部 truncate | PASS | `CACHE_CORRUPTED` / restart reject |

新增 M6 测试：`tests/m6_cache_faults.rs`。

---

## 8. Cache Trust Boundary：未声称的能力

当前 CacheStore 对 generation data 的核心完整性校验包含：

```text
manifest structure
expected committed length
filesystem existence/type
atomic publish/recovery boundary
```

当前**没有**为每个完整 generation 保存并在每次读取时验证全文件 cryptographic checksum。

因此：

```text
外部删除                 → 可检测
外部 truncate            → 可检测
额外尾部字节             → recovery 可收敛到 committed length
同长度、本地内容原位篡改 → 当前不能仅靠长度检测
```

这不是 Remote continuity fingerprint 的职责；continuity fingerprint 用于判断远程文件是否还能安全增量 append，而不是本地 cache 的全文件 MAC/checksum。

当前生产 trust boundary 应明确为：

```text
cache root = 0700
cache files = 0600
MCP 进程运行账户及其本地文件权限边界是可信边界
```

若未来威胁模型要求抵抗“同一 OS 账户或可绕过 Unix 权限的本地攻击者修改 cache 内容”，应新增独立设计：

```text
per-generation content checksum / authenticated manifest
```

M6 不应错误宣称当前已经检测该类同长度本地篡改。

---

## 9. Query / Snapshot 故障矩阵

| 场景 | 状态 | 预期 |
|---|---|---|
| Remote 新查询 | PASS | on-query refresh |
| cursor continuation | PASS | 不重新 SSH refresh |
| cursor 后 remote append | PASS | 当前 cursor 看不到新字节 |
| 新查询 after append | PASS | 看见增量数据 |
| rotation 后旧 cursor | PASS | pinned snapshot 保持稳定 |
| rotation 后旧 match_ref | PASS | 旧 generation TTL 内可读 |
| `get_log_context` Remote match | PASS | cache-only，0 SSH 正常路径 |
| Tail incomplete cache | PASS | `CACHE_SCOPE_EXCEEDED` |
| FromNow incomplete cache | PASS | `CACHE_SCOPE_EXCEEDED` |
| incomplete cache + no match | PASS | 不返回假阴性空数组 |

真实测试：`tests/m5_remote_query_live.rs`。

---

## 10. Multi-server Acceptance

M6-C 使用两个真正独立的 OpenSSH server fixture。

```text
Server A
  port 2222
  password auth
  host key A
  user logreader

Server B
  port 2225
  encrypted private key auth
  host key B
  user logreader_b
```

| 场景 | 状态 |
|---|---|
| A + B 同一查询 | PASS |
| 两台服务器相同相对文件名 `multi.log` | PASS |
| file identity 不冲突 | PASS |
| Password + Private Key 混合认证 | PASS |
| Local + A + B mixed query | PASS |
| global SSH semaphore 跨 connection 共享 | PASS |
| max connections = 1 时第二连接被拒绝 | PASS |
| A 不可用后 B 仍独立可查询 | PASS |
| A failure 不污染 B cache/query | PASS |

真实测试：`tests/m6_multi_server_live.rs`。

SSH workflow run `31190417993` 中：

```text
M2 transport                 PASS
M4 sync                      PASS
M5 query                     PASS
M6 symlink security          PASS
M6 two-server acceptance     PASS
research POC                 PASS
```

---

## 11. 当前正式 Gate

代码基线：

```text
5fb75f29c4148b29d0100e75a90174318709c1c9
```

Rust run `31190418118`：

```text
cargo fmt --all -- --check                          PASS
cargo clippy --locked --all-targets --all-features PASS
  -- -D warnings
cargo test --locked --all-targets --all-features   PASS
cargo build --release --locked --bins              PASS
```

Contracts run `31190417990`：

```text
v1 contracts PASS
v2 contracts PASS
```

SSH run `31190417993`：

```text
M2 + M4 + M5 + M6 security + M6 multi-server PASS
```

---

## 12. M6 剩余缺口

安全/故障/多服务器核心证据已经足够进入后半段。

剩余：

### M6-E Performance

- [ ] 可重复 benchmark harness。
- [ ] 100MB evidence。
- [ ] 1GB evidence。
- [ ] 10GB evidence。
- [ ] cold full bootstrap。
- [ ] tail bootstrap。
- [ ] unchanged continuity probe。
- [ ] append sync。
- [ ] cache-local scan。
- [ ] single-server concurrent query。
- [ ] dual-server concurrent query。
- [ ] 记录 transferred bytes / elapsed / CPU / RSS / disk。

### M6-F Production Docs

- [ ] README Remote mode。
- [ ] INSTALL Remote prerequisites。
- [ ] OPERATIONS cache / recovery / capacity / host-key rotation。
- [ ] read-only `log-reader` account hardening。
- [ ] SFTP-only / chroot 示例。
- [ ] Remote error troubleshooting。

---

## 13. 下一步

```text
M6-A Security audit          DONE
M6-B Fault / cache hardening DONE
M6-C Multi-server acceptance DONE
M6-D Evidence matrix         DONE
 ↓
M6-E Performance             NEXT
 ↓
M6-F Production docs
 ↓
M6 Final Gate
```

下一阶段不再扩展核心架构，只建立可重复性能证据与生产运维闭环。