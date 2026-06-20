# Log Query MCP 错误模型 v1

> 状态：M1 冻结草案

## 1. 目标

错误需要同时满足：

- AI 能稳定识别错误类别并决定重试、修改参数或重新搜索。
- 不暴露服务器绝对路径、系统调用参数、Rust 堆栈或凭证。
- 不把“无匹配”当成错误。
- 不要求客户端依赖不稳定的内部错误文本。

## 2. MCP 表达方式

业务和执行错误使用 MCP 工具结果：

```text
CallToolResult.isError = true
```

v1 在第一个文本内容块中返回一个紧凑 JSON 对象：

```json
{
  "code": "UNKNOWN_SOURCE",
  "message": "one or more requested log sources are unavailable",
  "retryable": false
}
```

对应线上的文本内容是该 JSON 对象的序列化字符串。v1 不在错误结果中返回服务器路径、inode、配置位置或底层系统调用文本。

协议解析、缺失必填字段和 JSON Schema 不合法等协议级错误可以由 MCP SDK 返回标准参数错误；进入工具业务处理后的错误必须使用本文定义的业务错误对象。

## 3. 错误对象

| 字段 | 类型 | 说明 |
|---|---|---|
| `code` | string | 稳定的机器错误代码 |
| `message` | string | 简洁、去敏后的英文说明 |
| `retryable` | boolean | 相同请求稍后重试是否可能成功 |

v1 不返回任意 `details` 对象，避免内部信息泄露和客户端依赖实现细节。

## 4. 稳定错误代码

| 代码 | retryable | 典型原因 | 客户端建议 |
|---|---:|---|---|
| `INVALID_ARGUMENT` | false | 参数范围、时间格式、字段关系错误 | 修改请求 |
| `UNKNOWN_SOURCE` | false | 来源不存在、禁用或不属于当前配置 | 重新调用 `list_log_sources` |
| `SOURCE_UNAVAILABLE` | true | 已配置文件暂时不存在、权限异常、跨挂载被拒绝 | 稍后重试或联系管理员 |
| `DEADLINE_EXCEEDED` | true | 排队或扫描超过 deadline | 缩小来源、时间或关键字范围 |
| `QUERY_CANCELLED` | true | 请求取消、会话结束或连接生命周期终止 | 按需重新执行 |
| `RESOURCE_LIMIT` | false | 扫描文件、字节、结果、上下文或响应限制 | 缩小查询范围 |
| `CURSOR_INVALID` | false | 游标过期、已消费、重启失效或条件变化 | 重新执行原始搜索 |
| `MATCH_REF_INVALID` | false | 引用过期、淘汰、重启失效或未知 | 重新搜索生成新引用 |
| `FILE_CHANGED` | true | 日志被轮转、替换、删除或截断 | 重新执行搜索 |
| `INTERNAL_ERROR` | true | 未预期内部错误 | 稍后重试并检查服务日志 |

## 5. 内部错误映射

| 内部情况 | v1 错误代码 |
|---|---|
| 空关键字、重复来源、`newest_first`、反向时间范围 | `INVALID_ARGUMENT` |
| `SourceRegistry` 找不到来源或来源禁用 | `UNKNOWN_SOURCE` |
| `openat2` 的不存在、权限、`NO_XDEV` 或普通文件校验失败 | `SOURCE_UNAVAILABLE` |
| 扫描 deadline | `DEADLINE_EXCEEDED` |
| CancellationToken 被触发 | `QUERY_CANCELLED` |
| 扫描文件、扫描字节、内容、响应或上下文限制 | `RESOURCE_LIMIT` |
| cursor 未知、过期、已消费、查询不一致 | `CURSOR_INVALID` |
| match_ref 未知、过期、已淘汰 | `MATCH_REF_INVALID` |
| device/inode 变化、保存偏移被截断、匹配位置失效 | `FILE_CHANGED` |
| 序列化失败、状态不变量破坏、任务 panic | `INTERNAL_ERROR` |

## 6. 示例

### 6.1 未知来源

```json
{
  "code": "UNKNOWN_SOURCE",
  "message": "one or more requested log sources are unavailable",
  "retryable": false
}
```

### 6.2 游标失效

```json
{
  "code": "CURSOR_INVALID",
  "message": "the search cursor is invalid or expired; run the search again",
  "retryable": false
}
```

### 6.3 文件轮转

```json
{
  "code": "FILE_CHANGED",
  "message": "the referenced log file changed; run the search again",
  "retryable": true
}
```

## 7. 日志记录

服务端内部日志可以记录更细的错误类别，但必须：

- 默认不记录完整关键字。
- 不在普通信息级日志记录完整绝对路径。
- 对运维所需路径使用受限调试日志或来源 ID + 相对文件标识。
- 生成内部 correlation ID 以关联客户端错误和服务端日志。

## 8. 兼容性

v1 可以新增错误代码，但不能改变已有代码的含义和 `retryable` 基本语义。

`message` 可以改进措辞；客户端必须依赖 `code`，不能依赖完整消息文本。
