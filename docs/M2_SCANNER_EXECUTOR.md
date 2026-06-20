# M2 有界扫描器与执行器

> 分支：`feat/scanner-executor`  
> 状态：实现完成，待 PR 评审

## 1. 目标

该切片负责把已经安全打开的单个普通日志文件转换为有界、可取消、可分页编排的搜索结果。

数据流：

```text
SafeFile / File
      ↓
ScanExecutor
      ↓ spawn_blocking + Semaphore
scan_reader(Read)
      ↓
ScanOutcome
```

扫描器不理解日志来源、MCP、时间范围或文件系统路径。

## 2. 字面量匹配

扫描器使用 KMP 状态机逐字节匹配：

- 支持关键字跨读取缓冲区。
- 不允许关键字跨日志行。
- 每条日志行只返回首次匹配。
- `case_sensitive=false` 仅执行 ASCII 大小写折叠。
- 中文等非 ASCII 内容按 UTF-8 字节精确匹配。
- 关键字不得为空、包含换行或超过 256 个 Unicode 字符。

不支持正则、glob、Shell 或查询语言。

## 3. 有界内存

扫描器只分配：

- 固定读取缓冲区。
- 单条日志行的有限预览窗口。
- 有界结果集合。
- 每条结果的有限文本内容。

硬上限：

```text
读取缓冲区：1 MiB
单行预览：1 MiB
结果数量：200
返回内容合计：16 MiB
扫描字节：64 GiB
```

v1 默认值更保守：

```text
读取缓冲区：64 KiB
单行预览：16 KiB
单页结果：50
返回内容合计：512 KiB
扫描字节：512 MiB
```

文件大小不会直接决定内存占用。

## 4. 超长行预览

匹配发生前，扫描器只保留有限的前置滑动窗口。首次匹配后，在单行预算内继续收集后续字节。

因此超长行结果满足：

- 预览包含首次匹配关键字。
- `content_truncated=true` 表示预览不是完整行。
- `original_line_bytes` 保存原始行长度。
- 预览若包含非法 UTF-8，使用替换字符并设置 `content_lossy=true`。

完整上下文读取不依赖该预览，而是使用行起点和匹配字节偏移重新定位。

## 5. 位置模型

`ScanPosition` 包含：

```text
byte_offset
line_number
```

默认位置：

```text
byte_offset = 0
line_number = 1
```

调用方若从非零位置扫描，必须先把 Reader seek 到对应字节，并确认该位置是日志行起点。扫描器据此返回绝对：

- `line_number`
- `line_start_offset`
- `match_byte_offset`

## 6. 安全续扫位置

`ScanOutcome.next_position` 只在确认位于完整行边界时返回。

典型情况：

| 停止原因 | next_position |
|---|---|
| 扫描完成 | `None` |
| 结果数量限制，停在换行后 | `Some` |
| 返回内容限制，停在换行后 | `Some` |
| 字节限制恰好位于换行后 | `Some` |
| 字节限制发生在半行 | `None` |
| 取消或 deadline 发生在半行 | `None` |
| 排队阶段取消或超时，尚未读取 | 起始位置 |

后续 cursor 层只能在 `next_position` 存在时直接续扫。半行停止时必须重新规划或终止分页，不能伪造偏移。

## 7. 停止原因

```text
Complete
ResultLimit
ScanByteLimit
ReturnedContentByteLimit
Cancelled
DeadlineExceeded
```

读取系统错误作为 `ScanError::Io` 返回，不伪装成正常停止。

## 8. ScanExecutor

同步普通文件读取运行在 Tokio `spawn_blocking` 中。

`ScanExecutor` 提供：

- Semaphore 并发限制。
- 最大并发硬上限 64。
- 排队阶段取消检查。
- 排队阶段绝对 deadline。
- 阻塞扫描阶段协作取消和 deadline。
- async Future 被丢弃时触发 CancellationToken。
- 扫描许可直到阻塞任务实际结束后才释放。

`spawn_blocking` 本身不是资源配额；真正的项目级配额由 Semaphore 提供。

## 9. 取消延迟

扫描循环每处理约 4 KiB 检查一次取消和 deadline，并在每次读取前再次检查。

取消为协作式：普通文件系统调用不能由安全 Rust 强制中止。取消延迟取决于：

- 当前文件系统调用完成时间。
- 读取缓冲区大小。
- 4 KiB 检查间隔。

目标 Linux 环境仍需执行真实磁盘和断连取消测试。

## 10. 自动化测试

已覆盖：

- 关键字跨读取缓冲区。
- UTF-8 中文关键字。
- ASCII 大小写折叠。
- 不跨行匹配。
- 超长行匹配预览。
- 非法 UTF-8 有损标记。
- CRLF 处理。
- 结果限制和安全续扫位置。
- 字节限制的行边界与半行差异。
- 非零起点的绝对行号和字节偏移。
- 预取消和过期 deadline。
- 无效关键字、限制和起点。
- 阻塞执行器正常扫描。
- 运行中取消。
- 排队中取消和 deadline。
- Reader 未启动时不发生读取。
- async Future 被丢弃后取消阻塞任务并释放许可。
- `SourceRegistry → SafeRoot → ScanExecutor → scan_reader` 真实文件链路。

## 11. 当前边界

- 扫描器尚未解析日志时间戳。
- 尚未编排多个文件和多个来源。
- 尚未创建 `match_ref` 和 cursor。
- `next_position` 的 Reader seek 和行边界复核由后续查询层负责。
- 完整 MCP JSON 响应大小限制尚未接入。

## 12. 下一步

下一切片：

```text
单/多来源查询编排
+ SourceFileSnapshot seek
+ 时间戳解析与 [start, end) 过滤
+ 稳定 oldest_first 排序
+ 页面级扫描资源汇总
```
