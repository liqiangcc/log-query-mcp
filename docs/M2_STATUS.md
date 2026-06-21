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

## 已完成切片四：多来源查询编排

- 查询参数、来源数量、关键字、时间范围和页面结果校验。
- 查询开始时刷新多来源候选文件快照。
- 查询级文件数、扫描字节、deadline 和取消限制。
- `SourceFileSnapshot` seek 与非零位置行边界复核。
- RFC 3339 与自定义固定前缀时间解析。
- `[start_time, end_time)` 过滤。
- 无时间和畸形时间保守返回。
- 固定容量结果集合保留全局最早结果。
- 时间、来源、文件、行号和偏移稳定排序。

详细说明见 [`M2_QUERY_ORCHESTRATION.md`](./M2_QUERY_ORCHESTRATION.md)。

## 已完成切片五：服务端 cursor 与 match_ref

- 有界 TTL 内存状态存储。
- cursor 绑定规范化查询、候选文件快照、排序水位线和累计资源。
- cursor 单次消费与查询条件校验。
- 跨页扫描文件、字节、页数和结果总数限制。
- 每条返回匹配注册短期不透明 `match_ref`。
- 文件身份、关键字语义和匹配位置保存在服务端。

## 已完成切片六：有限上下文读取

- `match_ref` 解析后重新经过 `SourceRegistry` 和 `SafeRoot`。
- 复核来源、配置范围、device/inode、文件大小、行边界和原关键字。
- 有界反向读取前置行。
- 有界向前读取匹配行和后续行。
- 超长匹配行围绕关键字生成有限预览。
- 上下文 I/O 运行在受限阻塞执行器中。
- 文件轮转、截断或原地改写后引用安全失效。

## 当前不包含

- MCP Server 与三个工具的正式接线。
- v1 工具错误 wire format 映射。
- 完整序列化 MCP JSON 响应大小限制。
- stdio 与 Streamable HTTP 正式二进制入口。
- MCP Inspector 和目标 AI 客户端验收。
- 目标 Linux 挂载、压力和性能验收。

## 下一切片

```text
rmcp Server
+ list_log_sources
+ search_logs
+ get_log_context
+ 稳定工具错误映射
+ 完整响应大小限制
+ stdio / Streamable HTTP 入口
```
