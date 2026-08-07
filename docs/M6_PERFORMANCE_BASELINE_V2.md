# Log Query MCP v2 M6 Performance Baseline

> 状态：large-file evidence complete；concurrency harness complete，最新 live rerun 被 GitHub Actions Billing 外部阻塞  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`  
> Large-file workflow：`M6 Performance`  
> Evidence run：`31195030284`  
> Evidence head：`fd3bdf08f16b8ad9b9272ed19f7952d204aba066`  
> Artifact：`m6-performance-31195030284` / `9000747578`  
> Artifact SHA256：`5a22387dd66168a97d4afa9092caa1f44725f9d31a5c0d36390956fe0e6730cc`

## 1. 定位

本文记录 M6-E 的可重复工程性能基线，用于验证：

- 大日志可以在有界内存/磁盘/网络范围内同步；
- unchanged 查询不会重复全量下载；
- append 只下载新增 payload 与 bounded continuity probes；
- 本地 Cache 查询完全不走 SSH；
- 10GB 级逻辑日志可采用 Tail bootstrap；
- 大量 bounded SFTP range read 不泄漏远端文件句柄；
- 单服务器和双服务器并发查询具备可重复 live benchmark harness。

这些数据**不是产品 SLA**。GitHub Hosted Runner 的 CPU、磁盘和调度会变化，绝对耗时只作为回归基线。

## 2. Benchmark 环境

本轮 large-file evidence 环境：

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

Workflow artifact 保存 environment snapshot、`/usr/bin/time -v`、metrics JSONL、filesystem usage 和 benchmark output。

## 3. 100MB Full Bootstrap

| Scenario | Wall time | Remote bytes | Cache write | Local scan |
|---|---:|---:|---:|---:|
| cold bootstrap | 4,945 ms | 104,923,136 | 104,857,600 | - |
| unchanged probe | 112 ms | 65,536 | 0 | - |
| local cache scan | 752 ms | 0 | 0 | 104,857,600 |
| 1 MiB append | 123 ms | 1,179,648 | 1,048,576 | - |

工程性质：

```text
cold read   = payload + 64 KiB continuity probe
unchanged   = 64 KiB only
append read = 1 MiB payload + 2 × 64 KiB probes
cache scan  = 0 remote bytes
```

## 4. 1GB Full Bootstrap

| Scenario | Wall time | Remote bytes | Cache write | Local scan |
|---|---:|---:|---:|---:|
| cold bootstrap | 48,640 ms | 1,073,807,360 | 1,073,741,824 | - |
| unchanged probe | 111 ms | 65,536 | 0 | - |
| local cache scan | 7,660 ms | 0 | 0 | 1,073,741,824 |
| 100 MiB append | 5,211 ms | 104,988,672 | 104,857,600 | - |

此前 1GB cold bootstrap 在大量 SFTP range read 后出现 `SftpProtocol`。根因是文件 handle 只依赖 Drop，没有等待协议级 CLOSE 完成。

修复后 `read_range()` 在每次 bounded read 完成后调用 `AsyncWriteExt::shutdown()`，并加入 300 次连续 range-read live 回归。修复后的 1GB benchmark 成功完成。

## 5. 10GB Tail Bootstrap

场景：

```text
logical remote size = 10 GiB
bootstrap mode      = tail
cached tail         = 64 MiB
```

fixture 使用 sparse prefix + dense tail，使 CI 不需要物理写满 10GB，但 SyncEngine 看到的逻辑大小仍为 10 GiB。

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

因此 10GB logical remote 不要求 10GB network transfer + 10GB local cache。

## 6. 已验证工程性质

### 6.1 Unchanged 不重复全量下载

100MB、1GB、10GB-tail 三档 unchanged refresh 均只读 `65,536` bytes continuity window。

### 6.2 Append 只传新增 payload + bounded probes

```text
1 MiB append   = 1,048,576 + 2 × 65,536 = 1,179,648 bytes
100 MiB append = 104,857,600 + 2 × 65,536 = 104,988,672 bytes
```

与实测完全一致。

### 6.3 Cache scan 不访问 SSH

三档 `cache_local_scan` 都满足：

```text
remote_bytes_read = 0
```

### 6.4 SFTP file handle 确定关闭

SSH live Gate 已验证同一 session 内：

```text
300 × open/read/shutdown
```

这覆盖了此前 >256 bounded reads 后可能暴露的 handle 生命周期问题。

### 6.5 SSH session 有全局上限

双服务器 live acceptance 已验证：`max_concurrent_ssh_connections=1` 时 Server A 占用唯一 permit，Server B 返回 `ConnectionLimit`；释放 A 后 B 可建立 session。

## 7. 并发 Query Benchmark Harness

已新增：

```text
tests/m6_concurrency_performance_live.rs
```

覆盖两个场景：

1. 单服务器 cache warm 后，4 个并发 `search_logs` 等价查询；
2. 双独立 SSH Server cache warm 后，Server A + Server B 并发查询。

测试复用 M6 两台真实 OpenSSH fixture，并输出：

```text
M6_CONCURRENCY_METRIC {"scenario":"single_server_4_queries",...}
M6_CONCURRENCY_METRIC {"scenario":"dual_server_2_queries",...}
```

该测试已接入只读 `SSH Transport` workflow。

### 当前证据状态

最新 concurrency live run 在 job 启动之前被 GitHub Actions 平台阻止，原因是账户 Billing/Spending Limit，不是 test failure。因此：

```text
harness implementation = DONE
workflow integration   = DONE
latest live elapsed-ms = BLOCKED (external Billing)
```

在 Billing 恢复后必须重跑 candidate commit，取得真实 elapsed/result evidence；在此之前不得编造 concurrency 数字，也不得把 M6 Final Gate 标成 PASS。

## 8. Evidence

Large-file workflow：

```text
M6 Performance
run: 31195030284
conclusion: success
```

Artifact：

```text
artifact id: 9000747578
name: m6-performance-31195030284
sha256: 5a22387dd66168a97d4afa9092caa1f44725f9d31a5c0d36390956fe0e6730cc
```

历史相关成功 gates：

```text
Rust CI       : fmt + clippy -D warnings + full tests + release build PASS
SSH live Gate : transport + 300 range reads + M4 + M5 + M6 security + 2-server + restart PASS
```

最新 candidate rerun 需在 GitHub Actions Billing 恢复后重新取得全绿证据。

## 9. M6-E 状态

```text
large-file benchmark              DONE
SFTP handle regression            DONE
single/dual concurrency harness   DONE
latest concurrency live evidence  BLOCKED by GitHub Actions Billing
```

从实现角度 M6-E 工作项已补齐；从 Release Gate 角度仍需要一次无平台阻塞的 live rerun。