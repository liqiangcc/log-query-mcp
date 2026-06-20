# M2 正式核心实现状态

## 已完成切片一：版本化配置模型

- Rust 2024 正式工程初始化。
- 应用层禁止 `unsafe`。
- v1 配置模型、默认值、硬上限和跨字段校验。
- `Cargo.lock` 与 `--locked` CI。

## 已完成切片二：安全来源注册表

- `SafeRoot`、Linux `openat2()` 和 `RESOLVE_NO_XDEV`。
- 有界目录发现和动态文件快照。
- `SourceRegistry`、不透明 file_id、device/inode/size 身份。
- 文件替换或截断后旧快照失效。

详细说明见 [`M2_SOURCE_REGISTRY.md`](./M2_SOURCE_REGISTRY.md)。

## 已完成切片三：有界扫描器与执行器

- 有界流式字面量扫描器。
- 绝对行号、行起点、匹配偏移和安全续扫位置。
- `spawn_blocking`、Semaphore、deadline 和协作取消。
- 安全文件到扫描结果的集成测试。

详细说明见 [`M2_SCANNER_EXECUTOR.md`](./M2_SCANNER_EXECUTOR.md)。

## 当前切片四：多来源查询编排

已实现：

- 查询参数、来源数量、关键字、时间范围和页面结果校验。
- 查询开始时刷新多来源候选文件快照。
- 查询级文件数、扫描字节、deadline 和取消限制。
- `SourceFileSnapshot` seek 与非零位置行边界复核。
- 只扫描查询开始时的文件大小。
- RFC 3339 与自定义固定前缀时间解析。
- `[start_time, end_time)` 过滤。
- 无时间和畸形时间保守返回。
- 固定容量最大堆保留全局最早结果。
- 时间、来源、文件、行号和偏移稳定排序。
- 页面结果和内容预算。
- 多来源、时间边界、全局 top-N、扫描字节和取消测试。

详细说明见 [`M2_QUERY_ORCHESTRATION.md`](./M2_QUERY_ORCHESTRATION.md)。

## 当前不包含

- cursor 和跨页面状态。
- match_ref 和上下文读取。
- MCP Server 与工具类型。
- 完整序列化 JSON 响应大小限制。
- 查询内并行文件扫描。
- 目标 Linux 挂载和性能验收。

## 下一切片

```text
cursor Store
+ 排序溢出结果
+ 未完成候选位置
+ 累计查询资源
+ match_ref 注册
```
