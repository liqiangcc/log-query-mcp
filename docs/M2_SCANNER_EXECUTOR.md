# M2 有界扫描器与执行器

> 状态：实现完成，待 PR 评审

## 目标

对单个普通日志文件执行有界字面量搜索，并将同步文件 I/O 与 Tokio 控制面隔离。

## 扫描器

`scan_reader` 只依赖标准库 `Read`。扫描过程中只保留固定读取缓冲区、当前行的有限匹配窗口和有限结果集合，内存不随文件大小线性增长。

搜索语义：

- 关键字非空且不能包含换行。
- 不解释为正则、glob 或 Shell。
- 匹配不跨日志行。
- 默认仅执行 ASCII 大小写折叠。
- 非 ASCII 关键字按 UTF-8 字节精确匹配。
- KMP 状态跨读取缓冲区保留。
- 每行记录首次匹配。

每条匹配保存：

```text
line_number
line_start_offset
match_byte_offset
original_line_bytes
content
content_truncated
content_lossy
```

超长行只返回包含匹配位置的有限预览。非法 UTF-8 使用有损文本并明确标记；完整 CRLF 行移除展示用尾部 `\r`。

## 资源限制

`ScanLimits` 控制：

```text
max_scan_bytes
max_results
max_line_bytes
max_returned_content_bytes
read_buffer_bytes
```

绝对硬上限与 v1 配置约束对齐。部署值由 `LimitsConfig` 转换，客户端不能突破服务端限制。

停止原因：

```text
Complete
ResultLimit
ScanByteLimit
ReturnedContentByteLimit
Cancelled
DeadlineExceeded
```

## 取消与 deadline

扫描器在每次读取前以及每处理约 4 KiB 后检查 `CancellationToken` 和绝对 deadline。普通文件读取采用协作取消；FIFO、Socket 和设备文件已由文件安全层拒绝。

## ScanExecutor

```text
异步查询
→ 等待 Semaphore
→ spawn_blocking
→ 同步 scan_reader
```

执行器保证：

- 全局扫描并发有界。
- 排队期间同时观察许可、取消和 deadline。
- 运行中的扫描占用许可，任务真正结束后释放。
- 等待结果的 Future 被丢弃时取消底层协作令牌。

## 自动验证

扫描器测试覆盖缓冲区边界、中文、大小写、跨行阻止、位置、超长行、非法 UTF-8、CRLF、各类限制、取消、deadline 和非法输入。

执行器测试覆盖正常扫描、运行中取消、排队取消、排队 deadline、Future 中止、许可释放和并发上限。

```text
Contracts CI: passed
cargo fmt --check: passed
cargo clippy --locked -D warnings: passed
cargo test --locked: passed
```

## 当前边界

- 尚未实现 cursor 恢复位置。
- 尚未编排多文件和多来源。
- 尚未解析和过滤日志时间戳。
- 尚未生成 `match_ref`。
- MCP/HTTP 客户端断连到取消令牌的传播仍需后续验证。

## 下一步

```text
多来源候选快照
→ 多文件顺序扫描
→ 时间过滤与 oldest_first 排序
→ 页级累计资源限制
→ match_ref 与 cursor
```
