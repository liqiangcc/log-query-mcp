# Log Query MCP v2 M7 ProxyCommand Performance Gate

> 状态：Harness implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25  
> 对照基线：[`M6_PERFORMANCE_BASELINE_V2.md`](./M6_PERFORMANCE_BASELINE_V2.md)

## 1. 目标

M7 性能 gate 验证 ProxyCommand 作为 SSH raw-stream adapter 后，不破坏 M6 已冻结的性能与资源不变量。

本 gate 不定义产品 SLA，也不要求 ProxyCommand 与 Direct 的绝对耗时完全相同。GitHub Hosted Runner 的 CPU、磁盘和调度具有波动，因此耗时用于回归比较；硬性不变量仍然是：

```text
bounded remote transfer
bounded cache growth
local cache scan = 0 remote bytes
bounded SFTP handles
bounded SSH concurrency
no orphan ProxyCommand process
no deadlock / unbounded buffering
```

## 2. 独立 Gate

```text
tests/m7_proxy_performance_live.rs
.github/workflows/m7-proxy-performance.yml
```

大 benchmark 不挂在普通产品 push 上；仅 benchmark harness 自身变化触发一次，最终候选通过 `workflow_dispatch` 显式重跑。

Evidence artifact：

```text
m7-proxy-performance-<run_id>
```

保存：

- runner environment；
- Direct / Proxy metrics JSONL；
- `/usr/bin/time -v`；
- filesystem usage；
- benchmark output。

## 3. Direct vs Proxy Connection Setup

同一 OpenSSH fixture、同一账号、同一 strict known_hosts、同一 password auth 下：

```text
Direct: TcpStream -> russh -> auth -> SFTP
Proxy : /usr/bin/nc {host} {port} -> russh -> auth -> SFTP
```

分别连续执行 5 次：

```text
open_reader
read_range(6, 5)
close
```

输出：

```text
M7_TRANSPORT_PERF_METRIC {"scenario":"direct_setup",...}
M7_TRANSPORT_PERF_METRIC {"scenario":"proxy_setup",...}
```

目前不设置毫秒级 hard threshold；Billing 恢复后先取得同 runner 环境下的 paired evidence，再判断是否需要合理的回归阈值。

## 4. 300 Range Read Regression

在一个已建立的 ProxyCommand SSH/SFTP session 内执行：

```text
300 x read_range(offset=6, length=5)
```

每次必须返回：

```text
world
```

目的：复验 M6 曾发现的 SFTP File handle 生命周期问题在 ProxyCommand stream 下不会复发。

该测试要求每次 bounded read 继续完成协议级 file shutdown；ProxyCommand 不得改变 SFTP handle lifecycle。

## 5. Direct + Proxy Concurrency

单个 `SshConnectionManager`：

```text
max_concurrent_ssh_connections = 4
```

并行运行：

```text
2 x Direct open/read/close
2 x Proxy  open/read/close
```

要求四条连接都完成，证明：

- Direct 与 Proxy 共用全局 SSH semaphore；
- Proxy child 不绕过 concurrency limit；
- 并发不存在 deadlock；
- Proxy 不阻塞独立 Direct session。

## 6. Large-file Paired Profiles

每个 profile 在同一个 OpenSSH fixture 上依次运行：

```text
Direct
ProxyCommand
```

两者使用相同 logical file、bootstrap policy、append payload、cache limits 和 sync byte budget。

### 6.1 100 MiB Full

```text
logical size  = 100 MiB
bootstrap     = full
append        = 1 MiB
```

### 6.2 1 GiB Full

```text
logical size  = 1 GiB
bootstrap     = full
append        = 100 MiB
```

### 6.3 10 GiB Logical Tail

```text
logical size  = 10 GiB
bootstrap     = tail(64 MiB)
append        = 1 MiB
```

10 GiB fixture 与 M6 一样使用 sparse prefix + dense tail，不要求 CI 物理写满 10 GiB。

## 7. Hard Transfer Invariants

M7 沿用 M6 的精确网络/缓存约束。

Cold bootstrap：

```text
remote read = cached payload + <= 64 KiB continuity probe
cache write = cached payload
```

Unchanged：

```text
remote read <= 64 KiB
cache write = 0
```

Incremental append：

```text
remote read <= append payload + 2 x 64 KiB probes
cache write = append payload only
same generation
```

Local cache scan：

```text
remote bytes = 0
```

这些是 gate 的 correctness/resource assertions，不依赖 runner 性能波动。

## 8. M6 对照数字

M6 已有成功 large-file evidence run `31195030284`，可作为 historical Direct baseline：

```text
100 MiB full cold bootstrap : 4,945 ms
1 GiB full cold bootstrap   : 48,640 ms
10 GiB tail(64 MiB) cold    : 3,082 ms

unchanged probe             : ~111-112 ms / 65,536 remote bytes
1 MiB append                : 1,179,648 remote bytes
100 MiB append              : 104,988,672 remote bytes
cache local scan            : 0 remote bytes
```

这些数字不是 M7 PASS threshold。M7 必须在当前候选和当前 runner 上同时产出 Direct + Proxy paired metrics 后再做性能判断。

## 9. Process Lifecycle Gate

workflow 在：

- transport benchmark 后；
- 每一个 Direct/Proxy large profile 后；
- workflow 收尾；

都检查目标：

```text
/usr/bin/nc 127.0.0.1 2235
```

不得存在 orphan ProxyCommand helper。

这与 M7 Failure Matrix 的 child cleanup 证据互补：Failure Matrix 验证故障/取消路径；Performance Gate 验证大量正常连接、同步、range read 后的正常回收路径。

## 10. Metrics

Large-file：

```text
M7_PERF_METRIC {
  transport,
  profile,
  scenario,
  remote_size_bytes,
  elapsed_ms,
  remote_bytes_read,
  cached_bytes_written,
  cache_disk_bytes,
  bytes_scanned
}
```

Transport：

```text
M7_TRANSPORT_PERF_METRIC {
  scenario,
  samples,
  elapsed_ms
}
```

## 11. 当前执行状态

GitHub 已识别 workflow：

```text
workflow = M7 Proxy Performance
run      = 31380836168
head     = 8d116de693f2ee05381b429944e4f5033533c150
job      = proxy-performance
result   = failure
steps    = null
```

runner 没有执行任何 step，与 Issue #23 的 GitHub Actions Billing / Spending Limit blocker 一致。

因此当前只能记录：

```text
performance harness     IMPLEMENTED
workflow recognition    CONFIRMED
actual metrics          NONE
M7 performance PASS     NO EVIDENCE
```

## 12. 完成条件

- [ ] rustfmt / cargo check PASS。
- [ ] Direct 5-session setup evidence。
- [ ] Proxy 5-session setup evidence。
- [ ] Proxy 300 range reads PASS。
- [ ] 2 Direct + 2 Proxy concurrency PASS。
- [ ] 100 MiB Direct + Proxy paired profile PASS。
- [ ] 1 GiB Direct + Proxy paired profile PASS。
- [ ] 10 GiB logical-tail Direct + Proxy paired profile PASS。
- [ ] unchanged <= 64 KiB remote read。
- [ ] incremental transfer bounded by payload + probes。
- [ ] cache-local scan = 0 remote bytes。
- [ ] no orphan ProxyCommand helper。
- [ ] no unexplained memory/disk/network regression。

真实 gate 完成前，M7 performance 只能标记 `HARNESS IMPLEMENTED / EXECUTION BLOCKED`，不能标记 PASS 或 production-ready。
