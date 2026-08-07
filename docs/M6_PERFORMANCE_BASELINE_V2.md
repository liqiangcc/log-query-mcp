# Log Query MCP v2 M6 Performance Baseline

> 状态：M6-E large-file benchmark evidence complete  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`  
> Benchmark workflow：`M6 Performance`  
> Evidence run：`31195030284`  
> Evidence head：`fd3bdf08f16b8ad9b9272ed19f7952d204aba066`  
> Artifact：`m6-performance-31195030284` / `9000747578`  
> Artifact SHA256：`5a22387dd66168a97d4afa9092caa1f44725f9d31a5c0d36390956fe0e6730cc`

## 1. 定位

本文记录 M6-E 的可重复工程性能基线。

这些数据用于回答：

- 大日志是否可以在有界内存/磁盘/网络范围内同步；
- unchanged 查询是否避免重复全量下载；
- append 是否只下载新增 payload 与 bounded continuity probes；
- 查询本地 Cache 时是否完全不走 SSH；
- 10GB 级逻辑日志是否可以采用 Tail bootstrap，而不必完整下载到本地；
- 大量 bounded SFTP range read 是否会泄漏远端文件句柄。

这些数据**不是产品 SLA**。GitHub Hosted Runner 的 CPU、磁盘和调度会变化，绝对耗时只能作为当前实现的工程基线。

---

## 2. Benchmark 环境

本轮环境：

```text
OS: Ubuntu 24.04
Architecture: x86_64
CPU: 2 vCPU, AMD EPYC 7763
Filesystem: ext4
rustc: 1.97.1
cargo: 1.97.1
SSH server: local OpenSSH 9.6p1 fixture
Transport: russh 0.62.5 + russh-sftp 2.3.0
```

Workflow 同时保存：

- environment snapshot；
- `/usr/bin/time -v` 输出；
- metrics JSONL；
- filesystem usage；
- benchmark stdout/stderr。

因此 CPU / peak RSS / wall time 等原始证据保存在 workflow artifact 中，而不是在文档里固化成 SLA。

---

## 3. 100MB Full Bootstrap

Logical remote file：100 MiB。

| Scenario | Wall time | Remote bytes | Cache write | Local scan |
|---|---:|---:|---:|---:|
| cold bootstrap | 4,945 ms | 104,923,136 | 104,857,600 | - |
| unchanged probe | 112 ms | 65,536 | 0 | - |
| local cache scan | 752 ms | 0 | 0 | 104,857,600 |
| 1 MiB append | 123 ms | 1,179,648 | 1,048,576 | - |

关键性质：

```text
cold read   = payload + 64 KiB continuity probe
unchanged   = 64 KiB only
append read = 1 MiB payload + 2 × 64 KiB continuity probes
cache scan  = 0 remote bytes
```

---

## 4. 1GB Full Bootstrap

Logical remote file：1 GiB。

| Scenario | Wall time | Remote bytes | Cache write | Local scan |
|---|---:|---:|---:|---:|
| cold bootstrap | 48,640 ms | 1,073,807,360 | 1,073,741,824 | - |
| unchanged probe | 111 ms | 65,536 | 0 | - |
| local cache scan | 7,660 ms | 0 | 0 | 1,073,741,824 |
| 100 MiB append | 5,211 ms | 104,988,672 | 104,857,600 | - |

此前 1GB cold bootstrap 在大量 SFTP range read 后会出现 `SftpProtocol`。根因是 `russh-sftp` 文件 handle 只依赖 Drop，没有等待协议级 CLOSE 完成。

修复后 `read_range()` 在每次 bounded read 完成后调用 `AsyncWriteExt::shutdown()`，并增加 300 次连续 range-read 实机回归测试。修复后的 1GB benchmark 成功完成。

---

## 5. 10GB Tail Bootstrap

10GB 场景使用：

```text
logical remote size = 10 GiB
bootstrap mode      = tail
cached tail         = 64 MiB
```

fixture 使用 sparse prefix + dense tail，使 CI 不需要真正占用 10GB 物理磁盘，但 SyncEngine 看到的逻辑文件大小仍然是 10 GiB。

| Scenario | Wall time | Remote bytes | Cache write / scan |
|---|---:|---:|---:|
| cold tail bootstrap | 3,082 ms | 67,174,400 | 67,108,864 written |
| unchanged probe | 112 ms | 65,536 | 0 |
| local cache scan | 477 ms | 0 | 67,108,864 scanned |
| 1 MiB append | 163 ms | 1,179,648 | 1,048,576 written |

Cached range：

```text
start = 10,670,309,376
end   = 10,737,418,240
size  = 67,108,864 bytes
```

这证明 10GB 级远程日志不要求：

```text
10GB remote
   ↓
10GB network transfer
   ↓
10GB local cache
```

而可以是：

```text
10GB logical remote
       ↓
64MiB tail bootstrap
       ↓
64MiB local cache
```

---

## 6. 已验证的工程性质

### 6.1 Unchanged 不会重复全量下载

100MB、1GB、10GB-tail 三档 unchanged refresh 均只读取：

```text
65,536 bytes
```

即一个 bounded continuity fingerprint window。

### 6.2 Append 只传新增 payload + bounded probes

1MiB append：

```text
1,048,576 + 2 × 65,536 = 1,179,648 bytes
```

100MiB append：

```text
104,857,600 + 2 × 65,536 = 104,988,672 bytes
```

与实测完全一致。

### 6.3 Cache scan 不访问 SSH

三档 `cache_local_scan`：

```text
remote_bytes_read = 0
```

查询路径保持：

```text
Remote Server
    ↓ sync
Local Cache
    ↓ scan
Query Engine
```

而不是查询时 remote grep/remote shell。

### 6.4 大量 SFTP range read 不泄漏 handle

SSH live Gate 新增：

```text
300 × open/read/shutdown
```

连续 bounded range read，并在同一 session 内完成。

这比 256 次更多，能够捕获此前大文件同步中远端 handle 未及时关闭的问题。

### 6.5 SSH session 仍受全局上限约束

现有双服务器 live acceptance 已验证：

```text
max_concurrent_ssh_connections = 1
```

时，Server A 占用唯一 permit 后，Server B 明确返回 `ConnectionLimit`；A 释放后 B 才能建立 session。

因此不存在无界 SSH session 创建。

---

## 7. 尚需补齐的 M6-E 证据

大文件容量基线已经完成。

按 M6 TODO，性能阶段还需要显式记录：

- 单服务器并发查询；
- 双服务器并发查询。

这些场景不需要重复使用 1GB fixture；应复用 M6 双服务器 live fixture，以较小日志测量并发 orchestration 开销、结果数量和全局 SSH semaphore 行为。

完成后 M6-E 才正式关闭。

---

## 8. Evidence

### Workflow

```text
M6 Performance
run: 31195030284
conclusion: success
```

### Artifact

```text
artifact id: 9000747578
name: m6-performance-31195030284
sha256: 5a22387dd66168a97d4afa9092caa1f44725f9d31a5c0d36390956fe0e6730cc
```

### Related regression gates

```text
Rust CI       : fmt + clippy -D warnings + full tests + release build PASS
SSH live Gate : transport + 300 range reads + M4 + M5 + M6 security + 2-server + restart PASS
```

---

## 9. 下一步

```text
M6-E large-file benchmark      DONE
M6-E concurrency benchmark     NEXT
M6-F production docs           PENDING
M6 Final Gate                  PENDING
```
