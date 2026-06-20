# Log Query MCP v1 实现基线

> 状态：M1 已完成，待 PR 评审合并  
> 日期：2026-06-20  
> 适用分支：`feat/initial-implementation`

本文用于把需求文档与技术预研结论对齐。正式实现和验收以本文、`MCP_API_V1.md`、`CONFIG_SCHEMA_V1.md`、`ERROR_MODEL_V1.md` 及对应机器可读 Schema 为准。

## 1. 首期范围

首期提供三个 MCP 工具：

```text
list_log_sources
search_logs
get_log_context
```

首期支持：

- 受控内网单实例部署。
- Linux 普通文本日志。
- 人工配置的显式日志文件。
- 人工配置的目录规则，按文件名后缀发现普通文件。
- 一个或多个日志来源的字面量子串搜索。
- UTF-8 中文和英文关键字。
- ASCII 大小写不敏感搜索。
- RFC 3339 查询时间范围。
- 搜索结果分页。
- 短期 `match_ref` 上下文读取。
- 严格的扫描、并发、返回内容和响应大小限制。

首期不支持：

- `newest_first` 真实搜索。
- 正则表达式、glob 查询或复杂查询语言。
- 压缩日志。
- 实时 tail/follow。
- 完整日志文件下载。
- Kubernetes、Loki 或 Elasticsearch。
- 多实例共享游标或引用。
- 客户端认证、来源授权和 MCP 自身 TLS。

## 2. 平台与部署基线

- 操作系统：Linux。
- 内核：`>= 5.6`。
- 传输：Streamable HTTP。
- 调试传输：stdio。
- 运行用户：独立非 root 用户。
- 日志目录：运行用户只读。
- 配置文件：运行用户不可修改。
- 默认监听：回环地址。
- 远程监听：必须显式配置，并由防火墙、安全组或反向代理限制内网访问。

低于 Linux 5.6 的环境不得静默退化为字符串前缀或一次性 `realpath` 路径校验。

## 3. 文件访问基线

客户端只能提交 `source_id`，不能提交：

- 服务器文件路径。
- 目录路径。
- 文件名模式。
- 文件标识与任意行号组合。

服务端文件访问必须：

1. 从管理员配置的日志根目录文件描述符开始。
2. 使用 Linux `openat2()`。
3. 启用：

```text
RESOLVE_BENEATH
RESOLVE_NO_SYMLINKS
RESOLVE_NO_MAGICLINKS
RESOLVE_NO_XDEV
```

4. 打开后通过 `fstat` 确认对象为普通文件。
5. 不调用 Shell、`grep`、`rg` 或 `find`。

需要查询不同挂载点时，管理员应把每个挂载点配置为独立来源根目录。

## 4. 搜索语义

- 关键字按字面量字节序列匹配。
- 关键字不能为空，最长 256 个 Unicode 字符。
- 不允许关键字包含换行符。
- 行号从 1 开始。
- `case_sensitive=false` 时，仅保证 ASCII 大小写折叠；非 ASCII 内容按 UTF-8 字节精确匹配。
- 首期结果顺序固定为 `oldest_first`。
- `order` 参数保留，但 v1 只接受 `oldest_first`，为后续兼容扩展保留字段。
- 同一文件内使用行号稳定排序。
- 有时间戳结果按绝对时间排序；相同时间戳使用来源顺序、文件顺序和行号稳定排序。
- 无时间戳结果位于可解析时间结果之后，不伪装成精确时间线。

## 5. 时间范围语义

查询时间参数使用 RFC 3339。

区间为：

```text
[start_time, end_time)
```

即开始时间包含，结束时间不包含。

日志来源可配置：

- RFC 3339 前缀时间戳。
- 固定格式前缀时间戳及来源级 UTC 偏移。

时间戳无法解析或明显畸形时：

- 不伪造时间。
- 匹配结果保守返回，`timestamp=null`。
- 服务端可记录格式告警。

文件修改时间只用于保守筛选或候选排序，不能作为单条日志事件时间返回。

## 6. 分页与上下文状态

`match_ref` 和 `cursor` 均为服务端生成的短期、不透明、随机 token。

共同语义：

- 不包含客户端可解析的路径、inode、行号或偏移。
- 具有 TTL 和全局容量上限。
- 服务重启后失效。
- 首期仅支持单实例使用。
- 无效、过期或已淘汰 token 返回工具错误。

`match_ref`：

- 绑定来源、规范化相对路径、文件身份和匹配位置。
- 只能用于 `get_log_context`。
- 文件轮转、替换或截断导致定位失效时，拒绝读取。

`cursor`：

- 绑定完整规范化查询条件、候选文件快照、扫描位置和累计资源使用。
- 后续页必须保持原查询条件不变。
- 每次成功返回下一页时生成新的 `next_cursor`。

## 7. 日志变化一致性

系统不锁定日志文件，不提供查询快照事务。

采用尽力而为模型：

- 搜索开始时记录候选文件身份和大小。
- 文件追加可以继续发生。
- 文件被替换时，通过 device/inode 识别。
- 文件被截断到保存位置之前时，游标或引用失效。
- 相同 inode 被原地覆盖的所有变化无法仅靠 inode 检测；关键位置应额外复核。
- 跨服务时间排序依赖日志时钟，不代表严格因果顺序。

## 8. v1 默认资源限制

| 限制 | 默认值 | v1 客户端可设置 |
|---|---:|---|
| 单次最大来源数 | 10 | 否 |
| 关键字最大长度 | 256 字符 | 否 |
| 单次最大扫描文件数 | 500 | 否 |
| 单页最大扫描字节数 | 512 MiB | 否 |
| 查询 deadline | 10 秒 | 否 |
| 默认单页结果数 | 50 | `max_results` 可设置 1–200 |
| 单页结果硬上限 | 200 | 不得超过上限 |
| 单条返回内容 | 16 KiB | 否 |
| 返回内容合计 | 512 KiB | 否 |
| 完整 MCP 响应 | 1 MiB | 否 |
| 单侧上下文行数 | 50 | `before_lines` / `after_lines` 可缩小 |
| 同时扫描任务数 | 4 | 否 |
| `match_ref` TTL | 10 分钟 | 否 |
| cursor TTL | 5 分钟 | 否 |

服务端可以通过受控配置调小或在代码硬上限内调大；客户端不能突破服务端生效上限。

## 9. v1 工具错误类别

业务错误使用 `CallToolResult.isError=true`，第一个文本内容块是符合 `tool-error-v1.schema.json` 的紧凑 JSON。

稳定代码：

```text
INVALID_ARGUMENT
UNKNOWN_SOURCE
SOURCE_UNAVAILABLE
DEADLINE_EXCEEDED
QUERY_CANCELLED
RESOURCE_LIMIT
CURSOR_INVALID
MATCH_REF_INVALID
FILE_CHANGED
INTERNAL_ERROR
```

错误中不得包含服务器绝对路径、底层系统调用参数、Rust backtrace、配置文件位置或凭证。

## 10. M1 冻结产物

- `REQUIREMENTS.md` v1.1。
- `schemas/mcp-tools-v1.schema.json`。
- `schemas/tool-error-v1.schema.json`。
- `schemas/log-query-mcp-config-v1.schema.json`。
- `docs/MCP_API_V1.md`。
- `docs/ERROR_MODEL_V1.md`。
- `docs/CONFIG_SCHEMA_V1.md`。
- `docs/adr/README.md` 及 ADR-0001 至 ADR-0006。
- `scripts/validate_contracts.py` 与 Contracts CI。

任何破坏兼容性的变更需要提高版本号或通过 ADR 明确记录。
