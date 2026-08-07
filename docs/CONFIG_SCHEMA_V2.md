# Log Query MCP 配置 Schema v2

> 状态：Draft  
> 日期：2026-08-07  
> 机器可读定义：[`schemas/log-query-mcp-config-v2.schema.json`](../schemas/log-query-mcp-config-v2.schema.json)  
> 架构方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)

## 1. 目标

v2 在不改变 v1 Local Source 安全语义的前提下，增加 SSH/SFTP Remote Source、本地持久缓存、增量同步和缓存 generation。

v2 配置仍遵循以下原则：

- 启动时整体解析和验证。
- 拒绝未知字段。
- 客户端不能读取、提交或覆盖 host、username、凭证、远程路径和缓存路径。
- SSH 只作为内部 Transport，不提供远程命令执行能力。
- Remote Source 必须先形成稳定本地 Snapshot，再进入现有查询引擎。

## 2. 完整示例

```json
{
  "version": 2,
  "connections": [
    {
      "connection_id": "test-server-01",
      "type": "ssh",
      "host": "10.0.0.10",
      "port": 22,
      "username": "log-reader",
      "auth": {
        "type": "password",
        "secret_ref": "TEST_SERVER_01_PASSWORD"
      },
      "host_key": {
        "known_hosts_file": "/home/user/.ssh/known_hosts"
      },
      "connect_timeout_millis": 5000,
      "operation_timeout_millis": 30000,
      "keepalive_seconds": 30
    }
  ],
  "sources": [
    {
      "source_id": "local-payment",
      "name": "本机支付服务日志",
      "service": "payment-service",
      "environment": "local",
      "backend": {
        "type": "local"
      },
      "root": "/var/log/payment-service",
      "files": ["application.log"]
    },
    {
      "source_id": "remote-order-test",
      "name": "远程订单服务测试日志",
      "service": "order-service",
      "environment": "test",
      "backend": {
        "type": "ssh",
        "connection_id": "test-server-01"
      },
      "root": "/data/log/order-service",
      "files": ["application.log", "application.log.1"],
      "directories": [
        {
          "path": "archive",
          "recursive": false,
          "include_suffixes": [".log", ".log.1"]
        }
      ],
      "sync": {
        "freshness": "on_query",
        "bootstrap": {
          "type": "tail",
          "bytes": 268435456
        },
        "allow_stale_on_error": false
      }
    }
  ],
  "cache": {
    "root": "/home/user/.cache/log-query-mcp",
    "max_bytes": 21474836480,
    "max_bytes_per_source": 5368709120,
    "retention_hours": 168,
    "max_generations_per_file": 4
  },
  "limits": {
    "max_sources_per_query": 10,
    "max_scan_files_per_query": 500,
    "max_scan_bytes_per_page": 536870912,
    "query_timeout_millis": 10000,
    "max_concurrent_scans": 4,
    "max_concurrent_ssh_connections": 4,
    "max_sync_bytes_per_query": 536870912,
    "max_remote_files_per_source": 500
  }
}
```

## 3. `connections`

v2 MVP 只定义 `type=ssh`。

每个连接必须包含：

- `connection_id`
- `host`
- `username`
- `auth`
- `host_key`

运行时必须额外验证：

- `connection_id` 全局唯一。
- Remote Source 引用的 `connection_id` 必须存在。
- Host Key Verification 实际启用，不能因为 `known_hosts` 文件缺失而降级为 accept-all。
- 日志和错误中不得输出凭证内容。

## 4. SSH 认证

### 4.1 Password

```json
{
  "type": "password",
  "secret_ref": "TEST_SERVER_PASSWORD"
}
```

普通 JSON 配置中不存在 `password` 字段。

MVP 的 `SecretResolver` 首先支持从环境变量或等价的受控本地 Secret 来源解析 `secret_ref`。后续可以扩展 OS Keychain、Vault 等实现，但不能改变配置中“不存明文密码”的原则。

### 4.2 Private Key

```json
{
  "type": "private_key",
  "key_file": "/home/user/.ssh/log-reader",
  "passphrase_secret_ref": "LOG_READER_KEY_PASSPHRASE"
}
```

私钥文件只在本地 MCP 进程中读取，不返回给 MCP 客户端。

## 5. Host Key Verification

```json
{
  "host_key": {
    "known_hosts_file": "/home/user/.ssh/known_hosts"
  }
}
```

v2 MVP 不提供：

```text
insecure=true
accept_all=true
strict_host_key_checking=false
```

或任何等价绕过字段。

Host Key 不匹配时返回稳定错误 `HOST_KEY_VERIFICATION_FAILED`。

## 6. `backend`

Local Source：

```json
{
  "backend": {
    "type": "local"
  }
}
```

Remote Source：

```json
{
  "backend": {
    "type": "ssh",
    "connection_id": "test-server-01"
  }
}
```

Local Source 继续使用现有 `openat2()` 安全打开模型。

Remote Source 不允许在客户端请求中覆盖 `connection_id`、host 或 root。

## 7. Remote `sync`

Remote Source 必须配置 `sync`。

MVP 新鲜度策略固定为：

```json
{
  "freshness": "on_query"
}
```

即每次新查询建立 Snapshot 前检查远程变化，有新增时只同步增量。

MVP 明确禁止静默使用过期缓存：

```json
{
  "allow_stale_on_error": false
}
```

当前 Schema 将该字段限制为 `false`；未来若引入 stale fallback，必须先扩展 MCP 响应契约，使客户端能明确看到 freshness / coverage 状态。

## 8. Bootstrap

Remote Source 必须显式选择首次缓存策略。

### `full`

```json
{
  "bootstrap": {
    "type": "full"
  }
}
```

同步完整文件历史。

### `tail`

```json
{
  "bootstrap": {
    "type": "tail",
    "bytes": 268435456
  }
}
```

仅同步文件末尾指定字节数。

### `from_now`

```json
{
  "bootstrap": {
    "type": "from_now"
  }
}
```

首次记录远程当前位置，之后只同步新增数据。

如果请求需要访问 Bootstrap 未覆盖的历史，必须返回 `CACHE_SCOPE_EXCEEDED`，不能把不完整查询结果伪装成“无匹配”。

## 9. Cache

只要配置中存在 Remote Source，顶层必须配置 `cache`。

```json
{
  "cache": {
    "root": "/home/user/.cache/log-query-mcp",
    "max_bytes": 21474836480,
    "max_bytes_per_source": 5368709120,
    "retention_hours": 168,
    "max_generations_per_file": 4
  }
}
```

v2 Draft 暂时要求这些容量字段显式填写，不在契约中冻结默认容量。正式 Release 前应根据基准测试确定是否增加默认值。

运行时额外约束：

- `max_bytes_per_source <= max_bytes`。
- cache root 必须由当前 MCP 进程安全创建/打开。
- cache 目录建议 `0700`，文件建议 `0600`。
- Cache Manifest 使用原子写入。
- 活动 Snapshot、cursor、`match_ref` 引用的 generation 不得被 GC。

## 10. 路径语义

`root` 仍然必须为 Linux 绝对路径；`files`、`directories[].path` 仍然只允许受控相对路径。

Local Source：

- 使用 v1 的 `openat2()` + `RESOLVE_BENEATH` / `NO_SYMLINKS` / `NO_MAGICLINKS` / `NO_XDEV`。

Remote Source：

- 不接受客户端路径。
- 使用管理员配置 root + 相对路径。
- 通过 SFTP `lstat` / 等价能力尽量拒绝软链接和非普通文件。
- 最终权限边界还必须依赖专用只读 SSH 用户、Unix 权限，以及生产环境推荐的 SFTP chroot。

## 11. Runtime 校验

JSON Schema 无法表达所有跨对象关系。实现必须额外验证：

1. `source_id` 全局唯一。
2. `connection_id` 全局唯一。
3. SSH Source 引用的 connection 必须存在且类型匹配。
4. Local Source 不得配置 `sync`。
5. Remote Source 必须配置 `sync`。
6. Remote Source 存在时必须配置 Cache。
7. `files` / `directories` 至少一个非空。
8. `max_bytes_per_source <= max_bytes`。
9. v1 已有 Limits 关系约束继续生效。
10. 所有配置值不得突破代码硬上限。

## 12. v1 / v2 兼容性

- `version=1`：继续使用当前 Local-only 配置和安全语义。
- `version=2`：使用新的 Backend / Connection / Cache 契约。
- 不在 `version=1` 中静默增加 SSH 字段。
- 解析器必须根据 `version` 选择严格的对应 Schema。

## 13. 相关 ADR

- [ADR-0003](./adr/0003-safe-file-access-with-openat2.md)：Local Source 文件访问边界。
- [ADR-0007](./adr/0007-support-remote-sources-via-local-cache.md)：Remote Source 先同步到本地 Cache。
- [ADR-0008](./adr/0008-use-ssh-sftp-without-remote-exec.md)：SSH/SFTP 安全边界。
- [ADR-0009](./adr/0009-use-cache-generations-and-query-snapshots.md)：Cache Generation 与 Snapshot。
- [ADR-0010](./adr/0010-use-on-query-sync-and-explicit-cache-coverage.md)：On-query Sync、Bootstrap 和 Coverage。
