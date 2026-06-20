# M2 Scanner / Executor 评审清单

## 扫描语义

- [x] 字面量匹配，不执行正则或 Shell。
- [x] 关键字可以跨读取缓冲区。
- [x] 关键字不能跨日志行。
- [x] 中文 UTF-8 匹配已测试。
- [x] 不区分大小写仅保证 ASCII。
- [x] 每行返回首次匹配。
- [x] CRLF 和无末尾换行文件可处理。

## 资源边界

- [x] 文件不整体加载到内存。
- [x] 读取缓冲区有硬上限。
- [x] 单行预览有硬上限。
- [x] 单页结果数有硬上限。
- [x] 返回内容总量有硬上限。
- [x] 扫描字节有硬上限。
- [x] 超长行预览仍包含匹配关键字。

## 定位与续扫

- [x] 返回绝对行号。
- [x] 返回行起始字节偏移。
- [x] 返回首次匹配字节偏移。
- [x] 支持非零扫描起始位置。
- [x] 只在完整行边界提供 `next_position`。
- [x] 半行停止不会伪造续扫位置。

## 并发与取消

- [x] 同步文件读取运行在 `spawn_blocking`。
- [x] Semaphore 限制同时扫描任务。
- [x] 排队阶段观察取消。
- [x] 排队阶段观察 deadline。
- [x] 扫描阶段协作检查取消和 deadline。
- [x] async Future 被丢弃后取消阻塞扫描。
- [x] 许可由阻塞任务实际结束后释放。

## 自动验证

- [x] Rustfmt 通过。
- [x] Clippy `-D warnings` 通过。
- [x] 单元测试通过。
- [x] `SourceRegistry → SafeRoot → ScanExecutor → scanner` 集成测试通过。
- [x] Contracts CI 通过。

## 后续切片

- [ ] Reader seek 到 `ScanPosition` 并复核行边界。
- [ ] 单/多来源查询编排。
- [ ] 时间戳解析和时间范围过滤。
- [ ] 页面级总文件、字节和结果配额。
- [ ] cursor 与 `next_position` 绑定。
