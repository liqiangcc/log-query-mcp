# M2 查询编排切片状态

## 已完成

- 单来源和多来源真实日志文件查询。
- 每个查询重新建立安全文件候选快照。
- 页面级扫描文件数、扫描字节、结果数、内容字节和 deadline。
- RFC 3339 查询时间解析。
- `[start_time, end_time)` 过滤。
- RFC 3339 和自定义固定前缀日志时间戳解析。
- 无法解析时间的匹配保守返回。
- `oldest_first` 跨来源稳定排序。
- 只保留最早且满足内容预算的有界结果集合。
- 页面统计和内部安全 continuation。
- 半行字节限制明确不可续扫。
- Rustfmt、Clippy 和全量 Rust 测试通过。

## 未完成

- 客户端可见 cursor Store。
- 跨页查询条件和累计配额绑定。
- `match_ref` 和有限上下文读取。
- MCP Server 工具接线。
- 完整序列化响应大小限制。

## 下一切片

```text
SearchCursorStore
+ MatchReferenceStore
+ continuation 状态绑定
+ 搜索结果 match_ref
+ 有界上下文读取
```
