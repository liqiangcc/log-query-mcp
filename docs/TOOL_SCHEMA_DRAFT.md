# R-03 MCP 工具 Schema 草案

> 状态：预研草案  
> 适用分支：`spike/technical-research`

## 1. 目标

本预研项验证三个 MCP 工具的输入结构是否足够明确，使 AI 能够稳定选择工具、构造合法参数，并且不能通过客户端参数突破服务端限制。

首期工具：

- `list_log_sources`
- `search_logs`
- `get_log_context`

## 2. 设计原则

1. AI 只提交 `source_id`，不提交服务器文件路径。
2. 搜索内容始终按字面量子串处理，不解释为正则、glob、查询语言或 Shell 表达式。
3. 能在 JSON Schema 中表达的限制直接写入 Schema。
4. 服务端仍执行同等运行时校验，不能只信任客户端或 Schema。
5. 请求拒绝未知字段，避免拼写错误被静默忽略。
6. 常用参数提供稳定默认值，减少 AI 无必要地填写参数。
7. 引用和游标均为不透明字符串，客户端不得解析或构造其内部内容。

## 3. `list_log_sources`

### 输入

无输入参数。

### 输出

每个来源返回：

```json
{
  "source_id": "payment-test",
  "name": "支付服务测试环境",
  "description": "payment-service application logs",
  "service": "payment-service",
  "environment": "test",
  "tags": ["payment", "java"]
}
```

不返回：

- 绝对路径。
- 运行用户。
- 主机目录结构。
- 配置文件位置。

## 4. `search_logs`

### 输入草案

```json
{
  "source_ids": ["payment-test", "order-test"],
  "keyword": "traceId=abc123",
  "case_sensitive": false,
  "start_time": "2026-06-19T14:00:00+09:00",
  "end_time": "2026-06-19T15:00:00+09:00",
  "order": "oldest_first",
  "max_results": 50,
  "cursor": null
}
```

### Schema 限制

| 字段 | 约束 | 默认值 |
|---|---|---|
| `source_ids` | 1 至 10 项 | 无 |
| 单个 `source_id` | 1 至 128 字符 | 无 |
| `keyword` | 1 至 256 字符 | 无 |
| `case_sensitive` | 布尔值 | `false` |
| `start_time` | 可选，1 至 64 字符，`date-time` 格式提示 | `null` |
| `end_time` | 可选，1 至 64 字符，`date-time` 格式提示 | `null` |
| `order` | `oldest_first` 或 `newest_first` | `oldest_first` |
| `max_results` | 1 至 200 | `50` |
| `cursor` | 可选，1 至 512 字符 | `null` |

### 当前行为

- 支持一个或多个已知日志来源。
- 默认不区分大小写。
- 返回结构化匹配结果。
- 达到 `max_results` 时设置 `truncated=true`。
- 当前 POC 使用模拟数据。
- 当前 POC 尚未应用时间范围。
- 当前 POC 尚未实现游标分页。

## 5. `get_log_context`

### 输入草案

```json
{
  "match_ref": "match-7ac9",
  "before_lines": 10,
  "after_lines": 30
}
```

### Schema 限制

| 字段 | 约束 | 默认值 |
|---|---|---|
| `match_ref` | 1 至 512 字符 | 无 |
| `before_lines` | 0 至 50 | `0` |
| `after_lines` | 0 至 50 | `0` |

该工具不接受：

- 文件路径。
- `file_id + line_number` 的任意组合。
- 超过服务端上限的上下文行数。

## 6. 运行时校验

服务端必须再次校验：

- 日志来源数量。
- `source_id` 长度和存在性。
- 关键字长度。
- 最大结果数。
- `match_ref` 和 `cursor` 长度。
- 上下文行数。
- 未知字段。

JSON Schema 用于帮助客户端和 AI 正确调用工具，不构成安全边界。

## 7. AI 调用验证场景

后续使用 MCP Inspector 和实际 AI 客户端验证：

```text
查询 payment-test 中 requestId=abc123 的日志
查询 14:20 前后支付服务的 PaymentAuthException
同时查询订单和支付服务中的 traceId=abc123
读取该异常前 10 行和后 30 行
继续读取下一批结果
```

重点观察：

- 是否先调用 `list_log_sources` 获取合法来源。
- 是否把路径或服务名称错误地当成 `source_id`。
- 是否能理解关键字是字面量。
- 是否会填写超过限制的参数。
- 是否能从 `search_logs` 返回结果中正确提取 `match_ref`。

## 8. 待验证问题

- `format: date-time` 是否会被目标客户端实际用于参数校验。
- `cursor` 是否应与其他查询条件互斥，或只允许单独提交。
- 时间参数是否继续使用字符串，还是在应用层引入强类型时间。
- MCP 客户端对输出 Schema 和结构化结果的展示一致性。

## 9. 当前结论

工具输入已具备首期 POC 所需的明确性，并且主要硬限制已经同时进入 JSON Schema 和运行时校验。

R-03 还需完成 MCP Inspector 和目标 AI 客户端的真实调用验证，完成前保持“有限通过”。
