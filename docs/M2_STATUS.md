# M2 正式核心实现状态

## 已完成切片一：版本化配置模型

- Rust 2024 正式工程初始化。
- 应用层禁止 `unsafe`。
- `AppConfig`、日志来源、目录规则、时间戳规则和资源限制模型。
- JSON 未知字段拒绝。
- v1 默认值和代码硬上限。
- source_id、文本长度、重复项和路径规范校验。
- 资源限制跨字段关系校验。
- 从字符串和文件加载配置。
- 提交 `Cargo.lock`，CI 使用 `--locked`。

## 已完成切片二：安全来源注册表

- `SourceRegistry` 只注册启用来源。
- `SafeRoot` 持有管理员配置的来源 root FD。
- 文件和目录使用 `openat2()` 从 root FD 解析。
- 启用 `RESOLVE_BENEATH`、`RESOLVE_NO_SYMLINKS`、`RESOLVE_NO_MAGICLINKS` 和 `RESOLVE_NO_XDEV`。
- 打开后使用 `fstat`，只接受普通文件或目录。
- 启动时验证显式文件和目录规则。
- 有界目录发现和查询时文件身份快照。
- 文件替换或截断后，旧快照安全失效。

详细说明见 [`M2_SOURCE_REGISTRY.md`](./M2_SOURCE_REGISTRY.md)。

## 已完成切片三：有界扫描器与执行器

- 基于 `Read` 的有界流式字面量扫描器。
- KMP 匹配支持跨读取缓冲区，不跨日志行。
- UTF-8 中文、ASCII 大小写折叠、非法 UTF-8 和 CRLF。
- 超长行围绕首次匹配生成有限预览。
- 扫描字节、结果数、单行预览、返回内容和读取缓冲区限制。
- 保存绝对行号、行起点和匹配字节偏移。
- `ScanPosition` 和仅在完整行边界返回的安全 `next_position`。
- `spawn_blocking` 隔离同步文件 I/O。
- Semaphore 限制扫描并发。
- 排队和运行阶段均观察 CancellationToken 和绝对 deadline。
- async Future 被丢弃后取消阻塞任务，许可在任务结束后释放。
- `SourceRegistry → SafeRoot → ScanExecutor → scan_reader` 集成测试。
- Rustfmt、Clippy、单元/集成测试和 Contracts CI 全部通过。

详细说明见 [`M2_SCANNER_EXECUTOR.md`](./M2_SCANNER_EXECUTOR.md)。

## 当前不包含

- MCP Server 和工具 Schema 的 Rust 类型实现。
- 多文件和多来源查询编排。
- 时间戳解析和 `[start_time, end_time)` 过滤。
- `match_ref`、cursor 和上下文读取。
- 完整 MCP JSON 响应大小限制。
- 具备 mount 权限环境中的真实 `RESOLVE_NO_XDEV` 跨挂载测试。

## 下一切片

```text
单/多来源查询编排
+ SourceFileSnapshot seek
+ 时间过滤
+ oldest_first 稳定排序
+ 页面级资源汇总
```
