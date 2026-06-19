# R-06 阻塞扫描执行器、超时与取消预研

> 状态：核心 POC 已通过自动化测试  
> 实现：`src/scan_executor.rs`

## 1. 目标

日志文件扫描属于同步阻塞文件 I/O。R-06 用于验证该工作能否与 MCP/HTTP 异步控制面隔离，并严格限制并发查询数量，同时支持超时和取消。

本阶段重点验证：

- 扫描任务不直接占用 Tokio 核心异步工作线程。
- 并发扫描数量受服务端限制。
- 运行中的扫描能够通过取消令牌协作退出。
- 扫描器能够检查绝对截止时间。
- MCP 请求 Future 被丢弃时，后台扫描会收到取消信号。
- 文件句柄、并发许可和扫描内存在任务结束后释放。

## 2. 执行模型

```text
MCP / Axum 异步请求
        ↓
Semaphore 获取扫描许可
        ↓
Tokio spawn_blocking
        ↓
同步 scan_reader
        ↓
定期检查 CancellationToken 和 deadline
```

`ScanExecutor` 使用 `tokio::sync::Semaphore` 控制同时运行的扫描任务数量：

```rust
let executor = ScanExecutor::new(max_concurrent_scans)?;
let outcome = executor.scan(file, request).await?;
```

扫描许可被移动到阻塞闭包内部，因此只有在底层扫描任务真正结束后才会释放。

## 3. 为什么使用有界并发

`spawn_blocking` 负责把阻塞工作移出异步核心线程，但它本身不是项目级资源配额。

日志扫描可能同时消耗：

- 磁盘读取带宽。
- 页缓存。
- CPU 字符串匹配时间。
- 文件描述符。
- 返回结果内存。

因此项目仍必须使用独立 Semaphore 设置较小的全局扫描并发数。正式默认值应由真实服务器基准决定，而不是直接采用 Tokio 阻塞池的容量。

## 4. 取消模型

Rust 无法安全地强制终止正在执行普通同步代码的线程，因此采用协作取消：

1. MCP 层或调用方持有 `CancellationToken`。
2. `ScanRequest` 将令牌传给同步扫描器。
3. 扫描器在每次读取前以及每处理约 4 KiB 后检查取消状态。
4. 发现取消后返回 `ScanStopReason::Cancelled`。

`ScanExecutor` 还使用 `CancelOnDrop`：

- 正常完成时解除该保护。
- 如果等待扫描结果的 async Future 被取消或丢弃，保护对象在 Drop 中取消令牌。
- 已经启动的 `spawn_blocking` 任务不会被假定为自动停止，而是依赖扫描循环观察取消令牌并退出。

该模型适用于本项目，因为 R-04 已限定只读取普通文件，不允许 FIFO、Socket 或设备文件。普通文件读取通常会及时返回，扫描器随后即可检查取消状态。

## 5. 截止时间

`ScanRequest` 可以携带 `std::time::Instant` 截止时间。

扫描器在与取消相同的检查点判断：

```text
Instant::now() >= deadline
```

达到截止时间时返回：

```text
DeadlineExceeded
```

当前截止时间只约束已经进入扫描器的任务。后续需要补充：

- 等待 Semaphore 许可期间也观察取消和截止时间。
- MCP/HTTP 层设置统一请求 deadline。
- 记录实际超时超出量。

## 6. 已验证用例

自动化测试覆盖：

- 在阻塞执行器中完成正常日志扫描。
- 运行中的慢速 Reader 收到取消后及时退出。
- Semaphore 将四个并发请求限制为最多两个同步扫描任务。
- 所有 Reader 结束后活动任务计数恢复为零。
- 拒绝配置为零的最大并发数。
- 同步扫描器对预先取消和已过期 deadline 的处理。

严格 CI 已通过：

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

## 7. 当前结论

R-06 核心技术结论为 **CONDITIONAL GO**：

已验证：

- 阻塞文件扫描可以与 Tokio 异步控制面隔离。
- 并发扫描可以通过 Semaphore 严格限制。
- 扫描中的超时和取消可以协作生效。
- 取消不依赖强制终止线程。

仍需验证：

- 客户端断开是否能从 `rmcp` / Axum 请求生命周期传递到业务 CancellationToken。
- Future 被丢弃后的阻塞任务退出时间。
- 等待并发许可期间的取消和 deadline。
- 高负载扫描时 MCP ping、工具发现等控制面请求的延迟。
- 目标取消响应时间不超过 500 ms。
- 目标超时超出量不超过 1 秒。

## 8. 正式实现建议

正式服务建议采用：

```text
全局 ScanExecutor
+ 小规模 max_concurrent_scans
+ 每个请求独立 CancellationToken
+ 每个请求绝对 deadline
+ 扫描器内部协作检查
+ systemd/cgroup 的进程级 CPU、内存和 FD 限制
```

不要采用：

- 在 Axum/Tokio 核心线程中直接扫描大文件。
- 为每个查询无上限地创建 OS 线程。
- 仅取消 async Future，却不通知同步扫描循环。
- 依赖客户端提供的限制值作为服务端硬上限。

## 9. 下一步

R-06 后续工作应与真实文件和 MCP 请求整合：

1. `SafeRoot::open_regular_file` 获取只读普通文件。
2. 将 `File` 交给 `ScanExecutor`。
3. 将 MCP 请求取消映射到 `CancellationToken`。
4. 对等待许可和运行扫描分别测量延迟。
5. 使用持续写入和大文件样本执行并发基准。

随后进入 R-07：设计安全的服务端有状态 `match_ref`，使 `get_log_context` 可以读取匹配位置附近日志而不开放任意文件和行号访问。
