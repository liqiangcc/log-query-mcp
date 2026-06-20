# Log Query MCP 配置 Schema v1

> 状态：M1 冻结草案  
> 机器可读定义：[`schemas/log-query-mcp-config-v1.schema.json`](../schemas/log-query-mcp-config-v1.schema.json)

## 1. 配置目标

管理员通过 JSON 配置日志来源和服务端资源限制。客户端不能读取、提交或覆盖服务器路径配置。

配置加载规则：

- 启动时完整解析和校验。
- 拒绝未知字段。
- 任一来源不合法时，服务启动失败。
- 配置更新失败不得形成部分生效状态。
- v1 通过重启加载配置，不要求热更新。

## 2. 完整示例

```json
{
  "version": 1,
  "sources": [
    {
      "source_id": "payment-test",
      "name": "支付服务测试环境",
      "description": "payment-service application logs",
      "service": "payment-service",
      "environment": "test",
      "tags": ["payment", "java"],
      "enabled": true,
      "encoding": "utf-8",
      "root": "/var/log/payment-service",
      "files": [
        "application.log",
        "application.log.1"
      ],
      "directories": [
        {
          "path": "archive",
          "recursive": false,
          "include_suffixes": [".log", ".log.1"]
        }
      ],
      "timestamp_rule": {
        "type": "rfc3339",
        "prefix_bytes": 64
      }
    }
  ],
  "limits": {
    "max_sources_per_query": 10,
    "max_scan_files_per_query": 500,
    "max_scan_bytes_per_page": 536870912,
    "query_timeout_millis": 10000,
    "default_results_per_page": 50,
    "max_results_per_page": 200,
    "max_line_bytes": 16384,
    "max_returned_content_bytes": 524288,
    "max_response_bytes": 1048576,
    "max_context_lines_per_side": 50,
    "max_concurrent_scans": 4,
    "match_reference_capacity": 10000,
    "match_reference_ttl_seconds": 600,
    "cursor_capacity": 1000,
    "cursor_ttl_seconds": 300
  }
}
```

## 3. 顶层字段

| 字段 | 必填 | 说明 |
|---|---:|---|
| `version` | 是 | 配置格式版本，v1 固定为 `1` |
| `sources` | 是 | 1–100 个日志来源，`source_id` 全局唯一 |
| `limits` | 否 | 服务端资源限制；缺省时使用 v1 默认值 |

## 4. 日志来源

### 4.1 基本字段

| 字段 | 必填 | 约束 |
|---|---:|---|
| `source_id` | 是 | ASCII 标识，1–128 字符，字符集 `[A-Za-z0-9._-]` |
| `name` | 是 | 非空显示名称，最长 256 字符 |
| `description` | 否 | 默认空字符串，最长 1024 字符 |
| `service` | 是 | 非空服务名，最长 256 字符 |
| `environment` | 是 | 非空环境名，最长 128 字符 |
| `tags` | 否 | 最多 32 项，单项最长 64 字符，唯一 |
| `enabled` | 否 | 默认 `true`；禁用来源不出现在 MCP 列表中 |
| `encoding` | 否 | v1 固定为 `utf-8` |
| `root` | 是 | Linux 绝对目录路径，仅管理员可见 |
| `files` | 条件必填 | 规范化、相对 `root` 的显式文件路径 |
| `directories` | 条件必填 | 安全目录发现规则 |
| `timestamp_rule` | 否 | 来源级日志时间戳规则 |

`files` 和 `directories` 至少有一个非空。

### 4.2 路径规则

`root`：

- 必须是 Linux 绝对路径。
- 启动时以不跟随最终软链接的方式打开。
- 必须是目录。

`files[].path` 和 `directories[].path`：

- 必须是相对路径。
- 不允许空路径。
- 不允许绝对路径。
- 不允许 `.` 或 `..` 路径组件。
- 不允许通过软链接或 magic link 解析。
- 运行时仍必须通过 `openat2()` 校验，JSON Schema 正则不是安全边界。

### 4.3 目录规则

```json
{
  "path": "archive",
  "recursive": false,
  "include_suffixes": [".log", ".log.1"]
}
```

| 字段 | 必填 | 说明 |
|---|---:|---|
| `path` | 是 | `.` 表示来源根目录，或规范化相对目录 |
| `recursive` | 否 | 默认 `false` |
| `include_suffixes` | 是 | 1–32 个区分大小写的文件名后缀 |

目录发现：

- 不跟随软链接。
- 只返回普通文件。
- 发现数量受 `max_scan_files_per_query` 和实现硬上限控制。
- 结果按规范化相对路径稳定排序。

## 5. 时间戳规则

### 5.1 RFC 3339

```json
{
  "type": "rfc3339",
  "prefix_bytes": 64
}
```

解析日志行前缀中的 RFC 3339 token，例如：

```text
2026-06-19T14:20:03.125+09:00
2026-06-19T05:20:03Z
```

### 5.2 自定义固定前缀

```json
{
  "type": "custom",
  "prefix_bytes": 23,
  "format": "%Y-%m-%d %H:%M:%S%.3f",
  "default_offset_seconds": 32400
}
```

适用于不包含时区的固定前缀。`default_offset_seconds` 将本地时间转换为绝对时间。

约束：

- `prefix_bytes`：1–256。
- `format`：1–128 字符。
- UTC 偏移必须有效。
- 无法解析的日志时间保持未知，不伪造。

## 6. 资源限制

### 6.1 默认值

| 字段 | 默认值 |
|---|---:|
| `max_sources_per_query` | 10 |
| `max_scan_files_per_query` | 500 |
| `max_scan_bytes_per_page` | 512 MiB |
| `query_timeout_millis` | 10000 |
| `default_results_per_page` | 50 |
| `max_results_per_page` | 200 |
| `max_line_bytes` | 16 KiB |
| `max_returned_content_bytes` | 512 KiB |
| `max_response_bytes` | 1 MiB |
| `max_context_lines_per_side` | 50 |
| `max_concurrent_scans` | 4 |
| `match_reference_capacity` | 10000 |
| `match_reference_ttl_seconds` | 600 |
| `cursor_capacity` | 1000 |
| `cursor_ttl_seconds` | 300 |

### 6.2 关系约束

实现必须额外校验：

- `default_results_per_page <= max_results_per_page`。
- `max_returned_content_bytes < max_response_bytes`。
- 单条内容上限不得大于返回内容上限。
- 上下文返回内容上限不得大于完整响应上限。
- 所有数值必须大于零。
- 配置值不得突破代码中的绝对硬上限。

## 7. 启动失败条件

以下任一情况必须拒绝启动：

- 配置不是合法 JSON。
- `version` 不支持。
- 存在未知字段。
- 没有日志来源或来源数超过上限。
- `source_id` 重复或不合法。
- 来源没有文件或目录规则。
- 根目录不能安全打开。
- 显式文件不存在、不是普通文件或不能安全打开。
- 目录规则非法或发现结果超过硬上限。
- 时间戳规则不合法。
- 资源限制关系矛盾。

错误可以在本地服务日志中记录管理员配置问题，但 MCP 客户端错误不得暴露服务器绝对路径。

## 8. 配置兼容性

v1 兼容变更：

- 增加可选字段并提供默认值。
- 增加新的时间戳规则类型。
- 增加新的资源限制字段并提供默认值。

不兼容变更：

- 改变已有字段含义。
- 把相对路径改成客户端可提交路径。
- 删除字段或改变类型。
- 改变 `version=1` 的默认安全边界。

不兼容变更必须使用新的配置版本。
