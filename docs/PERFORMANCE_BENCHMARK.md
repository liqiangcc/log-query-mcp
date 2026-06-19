# R-10 性能基准执行指南

> 本文定义可复现方法，不填写未经实测的性能结论。

## 1. 必须记录的环境

每次基准同时记录：Git 提交、Rust 版本、CPU、内存、Linux 内核、文件系统、存储介质、日志大小、缓存状态、关键字频率、迭代次数和并发数。

```bash
git rev-parse HEAD
rustc --version
cargo --version
uname -a
lscpu
free -h
findmnt -T /tmp
```

## 2. 构建

```bash
cargo build --release --locked --bin log-query-benchmark
```

## 3. 生成确定性日志

生成 1 GiB 文件：

```bash
python3 research/scripts/generate_benchmark_log.py \
  --output /tmp/log-query-benchmark-1g.log \
  --size-mib 1024 \
  --keyword BENCHMARK_MATCH \
  --match-every 100000
```

生成 10 GiB 文件时，将 `--size-mib` 设置为 `10240`。大型基准文件不得提交到仓库。

## 4. 完整文件扫描

使用不存在的关键字，避免结果上限导致提前停止：

```bash
/usr/bin/time -v \
  target/release/log-query-benchmark \
  /tmp/log-query-benchmark-1g.log \
  __NO_MATCH__ \
  3
```

基准程序输出 JSON：

- 文件大小。
- 迭代次数。
- 总扫描字节数。
- 总匹配数。
- 总耗时。
- MiB/s。
- 最后一次停止原因。

完整扫描的停止原因应为 `Complete`。

## 5. 匹配场景

稀疏匹配：

```bash
target/release/log-query-benchmark \
  /tmp/log-query-benchmark-1g.log \
  BENCHMARK_MATCH \
  3
```

高频匹配应使用单独生成的数据，并记录是否因结果数量或返回内容上限提前停止。提前停止场景的吞吐量不能与完整文件扫描直接比较。

## 6. 缓存状态

连续读取通常会形成热缓存。每次报告必须明确是首次读取还是重复读取。

冷缓存测试只应在专用测试机或可销毁环境中执行，不能在共享日志服务器上通过修改全局内核缓存状态来制造样本。

## 7. 并发测试

分别测试 1、4、8 个并发查询。除总吞吐外，还需要记录：

- 单查询延迟。
- 峰值 RSS。
- CPU 使用率。
- 磁盘吞吐和等待时间。
- 文件描述符数量。
- 取消响应时间。

正式服务内部使用 Semaphore 限制并发，进程级并发测试不能替代 `QueryService` 集成测试。

## 8. 大量小文件

目录发现和多文件编排稳定后，增加 10,000 个小文件测试，分别测量：

- 候选文件发现时间。
- `openat2()` 安全打开成本。
- 候选快照内存。
- 无匹配和少量匹配耗时。
- 分页恢复成本。

当前命令行基准只覆盖单文件扫描器。

## 9. 取消和超时

集成测试应分别测量：

1. 已运行扫描收到取消。
2. 等待 Semaphore 时收到取消。
3. 查询达到 deadline。
4. HTTP 客户端断开。

候选目标：取消响应不超过 500 ms，超时超出量不超过 1 秒。只有在目标文件系统实测通过后，才能将其写入正式验收标准。

## 10. 结果保存

建议目录：

```text
research/results/YYYY-MM-DD-hostname/
```

保存：

- 基准 JSON。
- `/usr/bin/time -v` 输出。
- 环境信息。
- 测试命令。
- 原始日志生成参数。

结果表：

| 项目 | 数值 |
|---|---|
| Commit | |
| Rust | |
| Kernel | |
| CPU / Memory | |
| Filesystem / Storage | |
| File size | |
| Cache state | |
| Keyword frequency | |
| Iterations / Concurrency | |
| Throughput MiB/s | |
| Max RSS | |
| CPU user / system | |
| Stop reason | |
| Notes | |

## 11. R-10 完成条件

完成以下实测后再确定生产默认限制：

- 1 GiB 和 10 GiB 单文件无匹配。
- 稀疏匹配和高频匹配。
- 超长单行和非法 UTF-8。
- 1、4、8 并发查询。
- 10,000 个小文件。
- 日志持续追加。
- 查询中发生轮转、截断和删除。
- 运行中取消和排队中取消。
- systemd 内存、CPU 和文件描述符限制下运行。

基于结果调整最大扫描字节数、并发数、查询超时、响应大小和 systemd 资源限制。
