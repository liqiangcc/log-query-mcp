# Log Query MCP 配置 Schema v2

> 状态：Draft / M7 ProxyCommand contract pending implementation  
> 日期：2026-08-10  
> 机器可读定义：[`schemas/log-query-mcp-config-v2.schema.json`](../schemas/log-query-mcp-config-v2.schema.json)  
> 架构方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> ProxyCommand 方案：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)

> 注意：本文已定义 M7 的目标配置契约；机器可读 JSON Schema 与 Rust runtime config 需要在 M7 实现阶段同步落地。在二者完成前，`proxy` 字段属于目标契约，不代表当前二进制已经接受该字段。

## 1. 目标

v2 在不改变 v1 Local Source 安全语义的前提下，增加 SSH/SFTP Remote Source、本地持久缓存、增量同步、缓存 generation，以及可选的 ProxyCommand SSH 底层 Transport。

v2 配置继续遵循以下原则：

- 启动时整体解析和验证。
- 拒绝未知字段。
- 客户端不能读取、提交或覆盖 host、username、凭证、远程路径、缓存路径或 ProxyCommand 配置。
- SSH 只作为内部 Transport，不提供远程命令执行能力。
- ProxyCommand 只建立 SSH raw byte stream，不是通用命令执行接口。
- Remote Source 必须先形成稳定本地 Snapshot，再进入现有查询引擎。
- Direct TCP 是默认连接方式；ProxyCommand 是每个 SSH Connection 的可选底层连接方式。

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
      "proxy": {
        "type": "command",
        "program": "ncat.exe",
        "args": ["{host}", "{port}"]
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

如果运行环境可以直接访问 SSH Server，则省略 `proxy`：

```json
{
  "connection_id": "direct-server",
  "type": "ssh",
  "host": "10.0.0.20",
  "username": "log-reader",
  "auth": {
    "type": "password",
    "secret_ref": "DIRECT_SERVER_PASSWORD"
  },
  "host_key": {
    "known_hosts_file": "/home/user/.ssh/known_hosts"
  }
}
```

## 3. `connections`

v2 定义 `type=ssh`。

每个连接必须包含：

- `connection_id`
- `host`
- `username`
- `auth`
- `host_key`

可选：

- `port`
- `proxy`
- `connect_timeout_millis`
- `operation_timeout_millis`
- `keepalive_seconds`

运行时必须额外验证：

- `connection_id` 全局唯一。
- Remote Source 引用的 `connection_id` 必须存在。
- Host Key Verification 实际启用，不能因为 `known_hosts` 文件缺失而降级为 accept-all。
- 日志和错误中不得输出凭证内容。
- `proxy` 只能来自管理员服务配置，不能来自 MCP Client 请求。

## 4. SSH 认证

### 4.1 Password

```json
{
  "type": "password",
  "secret_ref": "TEST_SERVER_PASSWORD"
}
```

普通 JSON 配置中不存在 `password` 字段。

`SecretResolver` 从环境变量或等价受控 Secret 来源解析 `secret_ref`。后续可以扩展 OS Keychain、Vault 等实现，但不能改变配置中“不存明文密码”的原则。

### 4.2 Private Key

```json
{
  "type": "private_key",
  "key_file": "/home/user/.ssh/log-reader",
  "passphrase_secret_ref": "LOG_READER_KEY_PASSPHRASE"
}
```

私钥文件只在本地 MCP 进程中读取，不返回给 MCP 客户端。

### 4.3 ProxyCommand 与凭证隔离

ProxyCommand 不参与 SSH Authentication。

禁止向 ProxyCommand argv 或 stdout 注入：

```text
password
private key content
private key passphrase
SecretResolver value
```

ProxyCommand 建立 raw stream 后，认证仍由现有 SSH Transport 完成。

## 5. Host Key Verification

```json
{
  "host_key": {
    "known_hosts_file": "/home/user/.ssh/known_hosts"
  }
}
```

v2 不提供：

```text
insecure=true
accept_all=true
strict_host_key_checking=false
```

或任何等价绕过字段。

Host Key 不匹配时返回稳定错误 `HOST_KEY_VERIFICATION_FAILED`。

即使底层通过 ProxyCommand 建立连接，Host Key Verification 仍然以 SSH Connection 中的逻辑 `host` / `port` 为目标，不能把代理进程、Windows Host 或 localhost 当成远程服务器身份。

## 6. `proxy` / ProxyCommand

`proxy` 是 `SshConnectionConfig` 的可选字段。

省略 `proxy`：

```text
Direct TCP
```

配置：

```json
{
  "proxy": {
    "type": "command",
    "program": "ncat.exe",
    "args": ["{host}", "{port}"]
  }
}
```

表示：

```text
spawn program
↓
program stdin/stdout
↓
SSH raw byte stream
↓
SSH handshake/auth/host-key verification
↓
SFTP
```

### 6.1 `type`

v2 / M7 首期只允许：

```json
{
  "type": "command"
}
```

不定义 SOCKS、HTTP CONNECT、jump-host 等单独配置类型；这些能力如果需要，可以由管理员选择合适的 ProxyCommand helper 实现，后续再根据真实需求决定是否增加一等 Transport 类型。

### 6.2 `program`

`program` 是要直接启动的本地可执行程序：

```json
{
  "program": "ncat.exe"
}
```

实现必须使用等价于：

```text
Command::new(program).args(args)
```

的 argv 模型。

项目内部不得自动拼接：

```text
sh -c
bash -c
cmd /c
powershell -Command
```

或任意 Shell Command String。

### 6.3 `args`

`args` 是 argv 数组：

```json
{
  "args": ["{host}", "{port}"]
}
```

首期限制：

- 最多 64 项。
- 单项最大 4096 bytes/characters contract limit，代码硬上限不得更宽松。
- Placeholder 必须占据完整 argv 项。
- 不执行 Shell parsing、变量展开、管道、重定向或 glob。

### 6.4 Placeholder

M7 首期只支持：

```text
{host}
{port}
```

来源固定为当前 `SshConnectionConfig`。

不支持：

```text
{username}
{password}
{secret}
{source_id}
{remote_path}
${VAR}
%VAR%
任意表达式
```

MCP Client / AI 不能提供 Placeholder 值。

### 6.5 stdout / stderr

ProxyCommand stdout 必须是纯 SSH 底层协议字节流，不允许混入日志或提示信息。

stderr 只能用于内部诊断，并必须：

- 有界读取。
- 去敏。
- 不原样返回 AI。
- 不记录 Secret、完整敏感 argv 或任意协议 stdout。

### 6.6 生命周期

ProxyCommand Process 与 SSH Session 强绑定：

```text
connect timeout / cancellation / SSH failure / normal close
↓
close stream
↓
terminate child when needed
↓
wait child
↓
release SSH permit
```

不得遗留 orphan process。

## 7. `backend`

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

Remote Source 不允许在客户端请求中覆盖 `connection_id`、host、proxy 或 root。

ProxyCommand 属于 Connection Transport，不属于 Source Backend，因此不会增加新的 `backend.type`。

## 8. Remote `sync`

Remote Source 必须配置 `sync`。

新鲜度策略固定为：

```json
{
  "freshness": "on_query"
}
```

即每次新查询建立 Snapshot 前检查远程变化，有新增时只同步增量。

明确禁止静默使用过期缓存：

```json
{
  "allow_stale_on_error": false
}
```

当前 Schema 将该字段限制为 `false`；未来若引入 stale fallback，必须先扩展 MCP 响应契约，使客户端能明确看到 freshness / coverage 状态。

ProxyCommand 失败与 Direct TCP 失败使用同样的 fail-closed 原则。

## 9. Bootstrap

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

## 10. Cache

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

运行时额外约束：

- `max_bytes_per_source <= max_bytes`。
- cache root 必须由当前 MCP 进程安全创建/打开。
- cache 目录建议 `0700`，文件建议 `0600`。
- Cache Manifest 使用原子写入。
- 活动 Snapshot、cursor、`match_ref` 引用的 generation 不得被 GC。

ProxyCommand 不改变任何 Cache / Generation / Snapshot 语义。

## 11. 路径语义

`root` 仍然必须为 Linux 绝对路径；`files`、`directories[].path` 仍然只允许受控相对路径。

Local Source：

- 使用 v1 的 `openat2()` + `RESOLVE_BENEATH` / `NO_SYMLINKS` / `NO_MAGICLINKS` / `NO_XDEV`。

Remote Source：

- 不接受客户端路径。
- 使用管理员配置 root + 相对路径。
- 通过 SFTP `lstat` / 等价能力拒绝软链接和非普通文件。
- 最终权限边界继续依赖专用只读 SSH 用户、Unix 权限，以及生产环境推荐的 SFTP chroot。

ProxyCommand 不获得 Remote Source path，也不能被用于任意服务器文件读取。

## 12. Runtime 校验

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
11. `proxy.type` 若存在必须等于 `command`。
12. `proxy.program` 必须非空并满足长度硬限制。
13. `proxy.args` 数量和单项长度必须满足硬限制。
14. 只允许完整 argv 项 `{host}` / `{port}` Placeholder。
15. 未知 Placeholder 必须在启动时拒绝。
16. Credential 不允许作为 ProxyCommand Placeholder 或参数来源。
17. ProxyCommand 不得改变 Host Key Verification 目标。
18. ProxyCommand connection 仍然必须提供正常 `host`、`port`、`username`、`auth` 和 `host_key`。
19. ProxyCommand 配置不得从 MCP Client 请求动态注入或覆盖。

## 13. JSON Schema M7 目标

机器可读 Schema 在 M7 落地时应新增：

```json
{
  "$defs": {
    "ProxyCommandConfig": {
      "type": "object",
      "additionalProperties": false,
      "required": ["type", "program", "args"],
      "properties": {
        "type": { "const": "command" },
        "program": {
          "type": "string",
          "minLength": 1,
          "maxLength": 4096
        },
        "args": {
          "type": "array",
          "maxItems": 64,
          "items": {
            "type": "string",
            "maxLength": 4096
          }
        }
      }
    }
  }
}
```

并给 `SshConnectionConfig.properties` 增加可选：

```json
{
  "proxy": {
    "$ref": "#/$defs/ProxyCommandConfig"
  }
}
```

Schema 仍然保持 `additionalProperties=false`。

Placeholder 的“必须占据完整 argv”“只允许 `{host}` / `{port}`”更适合由 runtime validator 严格执行；如 JSON Schema 能清晰表达，也可增加 pattern 作为第一层防御，但不能只依赖 Schema。

## 14. v1 / v2 兼容性

- `version=1`：继续使用当前 Local-only 配置和安全语义。
- `version=2`：使用 Backend / Connection / Cache 契约，并允许可选 ProxyCommand。
- v2 中没有 `proxy` 时保持 Direct TCP 行为。
- 不在 `version=1` 中静默增加 SSH / ProxyCommand 字段。
- 解析器必须根据 `version` 选择严格的对应 Schema。
- 因 v2 尚未正式发布，M7 直接扩展 v2，不升级到 `version=3`。

## 15. 相关 ADR

- [ADR-0003](./adr/0003-safe-file-access-with-openat2.md)：Local Source 文件访问边界。
- [ADR-0007](./adr/0007-support-remote-sources-via-local-cache.md)：Remote Source 先同步到本地 Cache。
- [ADR-0008](./adr/0008-use-ssh-sftp-without-remote-exec.md)：SSH/SFTP 安全边界。
- [ADR-0009](./adr/0009-use-cache-generations-and-query-snapshots.md)：Cache Generation 与 Snapshot。
- [ADR-0010](./adr/0010-use-on-query-sync-and-explicit-cache-coverage.md)：On-query Sync、Bootstrap 和 Coverage。
- [ADR-0011](./adr/0011-use-russh-and-russh-sftp.md)：Remote Transport 使用 `russh` 与 `russh-sftp`。
- [ADR-0012](./adr/0012-use-proxy-command-as-ssh-stream-transport.md)：ProxyCommand 作为 SSH 底层字节流 Transport。
