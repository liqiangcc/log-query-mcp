# Log Query MCP API v1

> 状态：M1 冻结草案  
> 成功结果机器定义：[`schemas/mcp-tools-v1.schema.json`](../schemas/mcp-tools-v1.schema.json)  
> 工具错误机器定义：[`schemas/tool-error-v1.schema.json`](../schemas/tool-error-v1.schema.json)  
> 错误映射说明：[ERROR_MODEL_V1.md](./ERROR_MODEL_V1.md)

## 1. 通用约定

- 所有请求对象均拒绝未知字段。
- 所有字符串均为 UTF-8。
- 时间使用 RFC 3339。
- 日志行号从 1 开始。
- 服务器绝对路径永不出现在工具结果中。
- `match_ref`、`file_id` 和 `next_cursor` 均为不透明字符串。
- v1 搜索结果顺序只支持 `oldest_first`。
- 时间范围采用 `[start_time, end_time)`。
- 无匹配返回成功的空结果，不返回工具错误。

## 2. `list_log_sources`

### 作用

返回管理员配置且启用的日志来源。

### 请求

```json
{}
```

### 响应

```json
{
  "sources": [
    {
      "source_id": "payment-test",
      "name": "支付服务测试环境",
      "description": "payment-service application logs",
      "service": "payment-service",
      "environment": "test",
      "tags": ["payment", "java"]
    }
  ]
}
```

### 字段

| 字段 | 说明 |
|---|---|
| `source_id` | 后续搜索使用的唯一来源标识 |
| `name` | 面向 AI 和用户的显示名称 |
| `description` | 来源说明，可以为空字符串 |
| `service` | 服务或模块名 |
| `environment` | 环境名，例如 `dev`、`test` |
| `tags` | 辅助选择来源的标签 |

不得返回根目录、文件列表、运行账号或配置路径。

## 3. `search_logs`

### 作用

在一个或多个已配置日志来源中执行字面量子串搜索。

### 请求

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

### 参数

| 参数 | 必填 | 默认值 | 约束 |
|---|---:|---:|---|
| `source_ids` | 是 | 无 | 1–10 个，唯一，单项 1–128 字符 |
| `keyword` | 是 | 无 | 1–256 字符，不得包含换行 |
| `case_sensitive` | 否 | `false` | `false` 仅保证 ASCII 大小写折叠 |
| `start_time` | 否 | `null` | RFC 3339，包含边界 |
| `end_time` | 否 | `null` | RFC 3339，不包含边界 |
| `order` | 否 | `oldest_first` | v1 只接受 `oldest_first` |
| `max_results` | 否 | `50` | 1–200 |
| `cursor` | 否 | `null` | 服务端生成，最长 512 字符 |

### 游标续查

使用 `cursor` 时，以下字段必须与创建游标的原查询一致：

```text
source_ids
keyword
case_sensitive
start_time
end_time
order
max_results
```

游标过期、已消费、服务重启或查询条件变化时返回 `CURSOR_INVALID`。

### 搜索语义

- 关键字不是正则、glob、Shell 或查询语言。
- 客户端提交的路径样式文本只作为普通关键字。
- 非 ASCII 文本按 UTF-8 字节精确匹配。
- 可解析时间戳结果按绝对时间排序。
- 无时间戳或畸形时间戳的匹配保守返回，`timestamp=null`，并排在可解析时间结果之后。

### 响应

```json
{
  "results": [
    {
      "match_ref": "mref_7c9f2d70a99244c1b9c8848f0c9cd807",
      "source_id": "payment-test",
      "file_id": "file-payment-test-0",
      "file_name": "application.log",
      "line_number": 1842,
      "timestamp": "2026-06-19T14:20:03.125+09:00",
      "content": "traceId=abc123 PaymentAuthException",
      "content_truncated": false
    }
  ],
  "truncated": false,
  "next_cursor": null
}
```

### 响应字段

| 字段 | 说明 |
|---|---|
| `match_ref` | 用于 `get_log_context` 的短期引用 |
| `source_id` | 日志来源 |
| `file_id` | 来源内的不透明文件标识 |
| `file_name` | 文件名或来源内相对显示名，不是绝对路径 |
| `line_number` | 从 1 开始的匹配行号 |
| `timestamp` | RFC 3339 时间；无法确定时为 `null` |
| `content` | 匹配行的有限预览 |
| `content_truncated` | 是否因单行上限截断 |
| `truncated` | 是否未返回全部可用结果 |
| `next_cursor` | 可继续查询时返回的新游标，否则为 `null` |

当响应大小或其他资源限制阻止继续安全分页时，允许 `truncated=true` 且 `next_cursor=null`。

## 4. `get_log_context`

### 作用

通过 `search_logs` 返回的 `match_ref` 读取匹配位置前后的有限日志行。

### 请求

```json
{
  "match_ref": "mref_7c9f2d70a99244c1b9c8848f0c9cd807",
  "before_lines": 10,
  "after_lines": 30
}
```

### 参数

| 参数 | 必填 | 默认值 | 约束 |
|---|---:|---:|---|
| `match_ref` | 是 | 无 | 1–512 字符，只能来自 `search_logs` |
| `before_lines` | 否 | `0` | 0–50 |
| `after_lines` | 否 | `0` | 0–50 |

该工具不接受路径、`file_id`、行号或字节偏移。

### 响应

```json
{
  "source_id": "payment-test",
  "file_id": "file-payment-test-0",
  "file_name": "application.log",
  "start_line": 1832,
  "end_line": 1872,
  "lines": [
    {
      "line_number": 1832,
      "content": "..."
    }
  ],
  "truncated": false
}
```

`truncated=true` 表示请求的前后行、单行内容或响应内容因服务端限制未完整返回。

文件被轮转、替换、删除、截断或引用过期时返回 `MATCH_REF_INVALID` 或 `FILE_CHANGED`。

## 5. 工具错误

进入工具业务处理后的错误使用：

```text
CallToolResult.isError = true
```

第一个文本内容块是符合 `tool-error-v1.schema.json` 的紧凑 JSON 字符串：

```json
{
  "code": "UNKNOWN_SOURCE",
  "message": "one or more requested log sources are unavailable",
  "retryable": false
}
```

v1 稳定错误类别：

| 类别 | 典型原因 |
|---|---|
| `INVALID_ARGUMENT` | Schema、范围、时间或字段关系错误 |
| `UNKNOWN_SOURCE` | `source_id` 不存在或未启用 |
| `SOURCE_UNAVAILABLE` | 已配置文件或目录无法安全读取 |
| `DEADLINE_EXCEEDED` | 查询超过服务端 deadline |
| `QUERY_CANCELLED` | 请求被取消或连接生命周期结束 |
| `RESOURCE_LIMIT` | 文件数、扫描字节、结果数或响应限制 |
| `CURSOR_INVALID` | 游标未知、过期、已消费或条件不一致 |
| `MATCH_REF_INVALID` | 引用未知、过期或已淘汰 |
| `FILE_CHANGED` | 引用或游标对应文件被轮转、替换或截断 |
| `INTERNAL_ERROR` | 未预期内部错误 |

客户端依赖 `code`，不能依赖完整 `message` 文本。错误消息不得包含服务器绝对路径、系统堆栈或凭证。

## 6. 兼容性规则

v1 兼容变更：

- 增加可选响应字段。
- 扩展 `order` 枚举以支持新顺序。
- 增加新的日志时间戳规则。
- 增加新的错误代码。
- 调小部署默认限制，但不改变机器 Schema 硬上限。

需要新版本的变更：

- 删除或重命名字段。
- 改变时间区间边界语义。
- 改变默认大小写语义。
- 允许客户端提交服务器路径。
- 将无匹配改为工具错误。
- 改变错误对象的必填字段。
