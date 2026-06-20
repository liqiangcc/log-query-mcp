# M2 多来源查询编排

> 分支：`feat/query-orchestration`  
> 状态：实现中，待严格 CI 验证

## 1. 目标

本切片把已完成的来源注册表和单文件扫描器串成单页查询能力：

```text
QueryRequest
  ↓
SourceRegistry 选择来源
  ↓
查询开始时刷新候选文件快照
  ↓
SafeRoot 重新打开并验证文件身份
  ↓
ScanExecutor 有界扫描
  ↓
时间戳解析与 [start, end) 过滤
  ↓
全局 oldest_first 稳定排序
  ↓
QueryPage + QuerySummary
```

该层不直接处理 MCP 协议，也尚不创建 cursor 或 match_ref。

## 2. 请求校验

查询层校验：

- 来源列表非空、唯一且不超过服务端上限。
- 关键字为 1–256 个 Unicode 字符且不含换行。
- `max_results` 位于服务端范围内。
- 时间参数为 RFC 3339。
- 时间范围必须满足 `start < end`。
- 使用调用方 deadline 与服务端查询超时中较早的值。

## 3. 文件候选快照

每个查询开始时调用 `ConfiguredSource::snapshot_files()`：

- 重新验证显式文件。
- 重新执行目录发现，覆盖服务启动后产生的轮转文件。
- 合并、排序和去重候选。
- 记录 device、inode 和查询开始时文件大小。
- 对所有来源共同执行 `max_scan_files_per_query`。

查询只扫描快照大小以内的字节。查询期间追加的数据留给后续查询，避免同一页面的候选范围不断增长。

## 4. 文件扫描

每个候选文件：

1. 通过 `open_snapshot_file()` 重新验证身份。
2. 根据 `ScanPosition` seek。
3. 非零位置检查前一个字节为换行符。
4. 使用 `Read::take()` 把 Reader 限制到快照剩余字节。
5. 交给 `ScanExecutor`。
6. 结果或内部内容限制触发时，在安全行边界继续同一文件。

整个页面累计：

- 扫描文件数。
- 扫描字节数。
- 扫描行数。
- 原始匹配数量。
- 时间过滤数量。
- 最终合格匹配数量。

## 5. 时间戳解析

来源可选配置：

- RFC 3339 首个空白分隔 token。
- 固定字节前缀 + chrono 格式 + 可选固定 UTC 偏移。

时间解析不依赖搜索结果展示内容。查询层根据 `line_start_offset` 重新读取最多 256 字节前缀，因此匹配位于超长行末尾时仍可解析时间。

时间范围语义：

```text
[start_time, end_time)
```

处理规则：

- 有效且在范围内：保留。
- 有效但范围外：排除。
- 无时间：保守保留，`timestamp=None`。
- 日期形态明显但解析失败：保守保留并计入 malformed 统计。

## 6. 全局稳定排序

查询不会简单返回第一个文件的前 N 条。

扫描所有预算内候选时，使用固定容量最大堆保留全局最早的 `max_results` 条。排序键：

```text
有效绝对时间（升序）
→ 无时间结果置后
→ 请求中的来源顺序
→ 来源内稳定文件顺序
→ 行号
→ 匹配字节偏移
```

堆容量最多 200，内存不会随匹配总数增长。

## 7. 内容和扫描限制

扫描器内部按小批次返回候选。查询层最终再次执行：

- 页面结果数量上限。
- 页面返回内容字节上限。
- 页面扫描字节上限。
- 页面文件数量上限。
- 服务端 deadline。
- CancellationToken。

单个结果超过剩余页面内容预算时，在 UTF-8 字符边界截断并设置 `content_truncated=true`。

## 8. 页面停止原因

```text
Complete
ResultLimit
ReturnedContentByteLimit
ScanByteLimit
```

取消和 deadline 当前作为 `QueryError` 返回，因为 v1 MCP 错误模型要求它们成为工具错误，而不是带部分结果的成功响应。

## 9. 当前边界

- 尚未实现 cursor；当前页面若因结果或内容上限截断，只提供 `truncated` 和停止原因。
- 尚未缓存溢出结果供下一页使用。
- 尚未创建 match_ref。
- 尚未限制完整序列化 MCP JSON 大小。
- 扫描当前为查询内顺序编排；Semaphore 仍限制跨查询全局扫描并发。
- 同 inode 原地覆盖仍属于尽力而为一致性。

## 10. 下一步

```text
服务端 cursor 状态
+ 已排序溢出结果
+ 未完成候选位置
+ 累计页面资源预算
+ match_ref 注册
```
