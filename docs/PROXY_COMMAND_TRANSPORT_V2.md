# Log Query MCP v2：ProxyCommand Transport 设计

> 状态：Draft  
> 日期：2026-08-10  
> 目标版本：v2 / M7  
> 前置方案：`REMOTE_SSH_CACHE_DESIGN_V2.md`  
> 配置契约：`CONFIG_SCHEMA_V2.md`  
> 核心目标：允许 SSH/SFTP 连接通过管理员配置的 ProxyCommand 建立底层双向字节流，以支持 WSL、VPN、堡垒网络、宿主机代理等无法直接 TCP 连接目标服务器的环境。

---

## 1. 背景

Log Query MCP v2 已支持：

```text
AI
 │
 ▼
Log Query MCP
 │
 ▼
SSH / SFTP
 │
 ▼
Remote Server
 │
 ▼
Log Files
```

当前 SSH Transport 采用直接 TCP 连接：

```text
Log Query MCP
      │
      ▼
TCP(host:port)
      │
      ▼
SSH
```

这要求运行 Log Query MCP 的环境本身能够访问 SSH Server。

实际开发环境中存在一种常见场景：

```text
Windows Host
    │
    ├── 公司 VPN
    ├── 特殊路由
    ├── 内网访问权限
    └── 可以访问 Remote Server
          ▲
          │
WSL ──────X
```

例如：

```text
Windows:
10.20.30.40:22  ✓

WSL:
10.20.30.40:22  ✗
```

如果 Log Query MCP 运行在 WSL 中，直接 SSH Transport 无法访问服务器，但 Windows 宿主机实际上具备访问能力。

类似问题还可能出现在：

```text
WSL
Container
Sandbox
Corporate VPN
Jump Host
Zero Trust Tunnel
cloudflared
自定义 TCP Relay
```

因此需要增加 ProxyCommand Transport。

## 2. 核心目标

ProxyCommand 的目标不是增加远程命令执行能力，而是：

> 允许管理员指定一个本地程序，由该程序建立到目标 SSH Server 的双向字节流，然后继续复用现有 SSH/SFTP 协议栈。

目标架构：

```text
AI
 │
 ▼
Log Query MCP
 │
 ▼
SSH Transport
 │
 ├─────────────────────────────┐
 │                             │
 ▼                             ▼
Direct TCP               ProxyCommand
 │                             │
 │                       spawn process
 │                             │
 │                      stdin / stdout
 │                             │
 └──────────────┬──────────────┘
                │
                ▼
            SSH Protocol
                │
                ▼
          Authentication
                │
                ▼
       Host Key Verification
                │
                ▼
              SFTP
                │
                ▼
           Local Cache
                │
                ▼
           Query Engine
```

ProxyCommand 只改变：

```text
SSH 底层连接如何建立
```

不改变：

```text
SSH Authentication
Host Key Verification
SFTP
Remote Source
Sync Engine
Cache
Snapshot
Query Engine
MCP API
```

## 3. 非目标

ProxyCommand **不是通用命令执行能力**。

仍然禁止：

```text
ssh_exec
run_shell
run_command
execute_remote_command
arbitrary shell
remote grep
remote cat
upload
write
delete
deploy
sudo
service restart
```

同时禁止 MCP Client / AI 动态提交：

```text
program
command
args
host
port
environment
remote path
```

这些内容只能存在于管理员控制的服务配置中。

AI-facing MCP API 继续保持：

```text
list_log_sources
search_logs
get_log_context
```

不新增 ProxyCommand MCP Tool。

## 4. 设计原则

### 4.1 ProxyCommand 是 Transport

ProxyCommand 的职责只有：

```text
启动本地进程
↓
获得 stdin/stdout
↓
形成双向字节流
↓
交给 SSH Client
```

ProxyCommand 不负责：

```text
SSH Authentication
Host Key Verification
SFTP
日志读取
日志搜索
缓存
日志分析
```

### 4.2 stdout 必须是纯二进制 Transport

ProxyCommand：

```text
stdin  ← SSH Client 写入的数据
stdout → SSH Client 读取的数据
```

stdout 不允许包含：

```text
日志
提示信息
debug message
banner
JSON
错误文本
```

否则会直接破坏 SSH Protocol。

诊断信息只能通过 stderr 输出。

### 4.3 不通过 Shell 执行

禁止：

```text
sh -c "..."
bash -c "..."
cmd /c "..."
powershell -Command "..."
```

作为内部自动拼接机制。

实现必须直接：

```text
program + argv[]
```

创建进程。

例如：

```json
{
  "program": "ncat.exe",
  "args": [
    "{host}",
    "{port}"
  ]
}
```

而不是：

```json
{
  "command": "ncat.exe {host} {port}"
}
```

这样可以避免 Shell：

```text
解析
转义
变量展开
管道
重定向
命令注入
```

## 5. 配置设计

现有 `SshConnectionConfig` 保持不变，只增加可选 `proxy` 字段。

没有 `proxy` 时继续使用现有 Direct TCP。

### 5.1 Direct

现有配置继续有效：

```json
{
  "connection_id": "test-server-01",
  "type": "ssh",
  "host": "10.20.30.40",
  "port": 22,
  "username": "log-reader",
  "auth": {
    "type": "password",
    "secret_ref": "TEST_SERVER_PASSWORD"
  },
  "host_key": {
    "known_hosts_file": "/home/user/.ssh/known_hosts"
  }
}
```

等价于：

```text
proxy = none
```

连接路径：

```text
WSL
 ↓
TCP
 ↓
10.20.30.40:22
```

## 6. ProxyCommand 配置

建议：

```json
{
  "connection_id": "test-server-01",
  "type": "ssh",
  "host": "10.20.30.40",
  "port": 22,
  "username": "log-reader",
  "auth": {
    "type": "password",
    "secret_ref": "TEST_SERVER_PASSWORD"
  },
  "host_key": {
    "known_hosts_file": "/home/user/.ssh/known_hosts"
  },
  "proxy": {
    "type": "command",
    "program": "ncat.exe",
    "args": [
      "{host}",
      "{port}"
    ]
  }
}
```

执行效果：

```text
ncat.exe 10.20.30.40 22
```

但实现内部不能生成 Shell Command String，而应等价于：

```text
Command::new("ncat.exe")
    .arg("10.20.30.40")
    .arg("22")
```

## 7. WSL + Windows Host 场景

典型环境：

```text
┌──────────────── Windows ────────────────┐
│                                         │
│ VPN                                     │
│  │                                      │
│  ▼                                      │
│ 10.20.30.40:22 ✓                        │
│                                         │
│ ┌──────────── WSL ────────────────┐     │
│ │                                 │     │
│ │ log-query-mcp                   │     │
│ │      │                          │     │
│ │      ├── direct TCP ─────── X   │     │
│ │      │                          │     │
│ │      ▼                          │     │
│ │ ProxyCommand                    │     │
│ │      │                          │     │
│ │      ▼                          │     │
│ │ Windows executable ─────────────┼─────┼──► Server
│ │                                 │     │
│ └─────────────────────────────────┘     │
└─────────────────────────────────────────┘
```

例如：

```json
{
  "proxy": {
    "type": "command",
    "program": "ncat.exe",
    "args": [
      "{host}",
      "{port}"
    ]
  }
}
```

WSL 启动 Windows executable：

```text
WSL
 │
 ▼
ncat.exe
 │
 ▼
Windows Network Stack
 │
 ▼
Corporate VPN
 │
 ▼
SSH Server
```

因此 MCP 本身仍然运行在 WSL，不需要部署到 Windows。

## 8. Placeholder

v2 首期只允许两个 Placeholder：

```text
{host}
{port}
```

例如：

```json
{
  "args": [
    "--target",
    "{host}",
    "--port",
    "{port}"
  ]
}
```

运行时解析：

```text
{host} → SshConnectionConfig.host
{port} → SshConnectionConfig.port
```

暂不支持：

```text
{username}
{password}
{secret}
{source_id}
{remote_path}
任意环境变量表达式
```

Credential 不得通过 ProxyCommand argv 传递。

SSH Authentication 继续完全由现有 SSH Transport 负责。

## 9. Host Key Verification

使用 ProxyCommand 后，Host Key Verification 不能改变。

逻辑目标仍然是：

```text
host = 10.20.30.40
port = 22
```

而不是：

```text
proxy process
Windows Host
localhost
```

连接路径虽然变成：

```text
SSH Client
  ↓
ProxyCommand
  ↓
10.20.30.40:22
```

但 Host Key Verification 必须继续校验 `10.20.30.40:22` 对应的服务器身份。

因此 ProxyCommand 不能绕过：

```text
known_hosts
strict host key verification
```

ProxyCommand 成功建立网络连接但 Host Key 不匹配时，仍然返回：

```text
HOST_KEY_VERIFICATION_FAILED
```

并立即终止连接。

## 10. Authentication

ProxyCommand 不参与 SSH Authentication。

现有：

```text
Password
Private Key
Encrypted Private Key
SecretResolver
```

保持不变。

流程：

```text
ProxyCommand
      │
      ▼
raw stream
      │
      ▼
SSH handshake
      │
      ▼
Host Key Verification
      │
      ▼
Authentication
      │
      ▼
SFTP
```

因此 ProxyCommand 不应该获得：

```text
SSH password
private key
private key passphrase
SecretResolver value
```

## 11. Transport 抽象

建议把当前 `SshReadTransport` 中的 TCP 建连责任抽出。

目标结构：

```text
SshReadTransport
        │
        ▼
SshStreamConnector
        │
        ├── DirectConnector
        │
        └── ProxyCommandConnector
```

概念接口：

```rust
trait SshStreamConnector {
    async fn connect(
        &self,
        connection: &SshConnectionConfig,
    ) -> Result<ConnectionStream, SshTransportError>;
}
```

其中 `DirectConnector` 负责：

```text
TcpStream::connect(host, port)
```

而 `ProxyCommandConnector` 负责：

```text
spawn program
↓
stdin/stdout
↓
AsyncRead + AsyncWrite
```

随后统一进入 stream-based SSH client API。

## 12. Process 生命周期

ProxyCommand Process 必须与 SSH Session 强绑定。

状态关系：

```text
Proxy Process
     │
     ▼
SSH Session
     │
     ▼
SFTP Session
```

任意一个结束，都必须清理其他资源。

### 12.1 正常结束

SSH Session 关闭：

```text
close SFTP
↓
close SSH
↓
close proxy stdin/stdout
↓
terminate/wait proxy process
```

### 12.2 Connection Timeout

如果 ProxyCommand 启动，但 SSH handshake 没有在 `connect_timeout_millis` 内完成：

```text
cancel SSH connect
↓
terminate ProxyCommand
↓
wait child process
↓
释放 SSH semaphore
```

### 12.3 Cancellation

Query / Sync 被取消时不得遗留：

```text
powershell.exe
ncat.exe
cloudflared
ssh.exe
proxy helper
```

后台孤儿进程。

### 12.4 Early Exit

如果 ProxyCommand 在 SSH handshake 前退出，或者 stdout EOF，必须返回稳定 Transport Error，不得无限等待 SSH timeout。

## 13. stderr

ProxyCommand stdout 是协议流。

stderr 可以作为有限诊断信息使用，但必须满足：

```text
bounded
redacted
not returned raw
```

建议：

```text
MAX_PROXY_STDERR_BYTES = 64 KiB
```

超过后截断。

Public Error 不能返回：

```text
完整 argv
环境变量
Secret
Private Key path
任意敏感 stdout/stderr
```

内部日志可以记录：

```text
connection_id
proxy program basename
exit code
error category
duration
```

但仍需去敏。

## 14. 环境变量

首期原则：

> ProxyCommand 不自动注入 SSH Credential。

可以继承运行 Log Query MCP 所必需的基础进程环境，从而支持 WSL 调用 Windows executable。

但配置中不提供：

```text
secret_env
credential_env
dynamic_env_from_client
```

如未来确实需要显式环境变量，应单独设计 `env allowlist / SecretResolver`，不能通过任意配置直接存储 Secret。

## 15. 安全边界

### 15.1 AI 不得控制 ProxyCommand

MCP Client 请求中不能出现：

```text
proxy
program
args
host
port
```

ProxyCommand 只能来自管理员配置。

### 15.2 不允许动态 Shell

禁止把：

```text
host
port
connection_id
source_id
AI input
query keyword
```

拼入 Shell String。

只允许对 argv 中整个 Placeholder 做替换。

推荐：

```json
[
  "{host}",
  "{port}"
]
```

MVP 可以要求 Placeholder 占据完整 argv，以进一步减少解析复杂度。

### 15.3 ProxyCommand 不改变远程权限

远端仍然要求：

```text
Dedicated read-only SSH user
+
Unix permissions
+
SFTP-only where practical
```

ProxyCommand 不能把 Remote Source 变成远程 Shell。

## 16. 错误模型

建议增加内部错误分类：

```text
PROXY_COMMAND_NOT_FOUND
PROXY_COMMAND_START_FAILED
PROXY_COMMAND_EARLY_EXIT
PROXY_COMMAND_IO_FAILED
PROXY_COMMAND_TIMEOUT
PROXY_COMMAND_CANCELLED
```

对 AI-facing API 可以根据现有错误契约映射成：

```text
REMOTE_UNAVAILABLE
SOURCE_UNAVAILABLE
```

并通过稳定 `reason` / diagnostic category 区分。

如果决定直接增加 Public Error Code，则必须同步更新：

```text
ERROR_MODEL_V1/V2
tool-error schema
contract validation
tests
```

不得把 raw OS error、raw stderr、full command line 直接返回给 AI。

## 17. Connection Limit

现有 `max_concurrent_ssh_connections` 继续作为总限制。

ProxyCommand Connection 同样占用一个 SSH Permit：

```text
acquire SSH permit
↓
spawn ProxyCommand
↓
SSH
↓
SFTP
↓
release permit
```

因此不会因为 ProxyCommand 绕过现有并发控制。

## 18. Timeout

首期不增加新的用户配置 Timeout。

继续使用：

```text
connect_timeout_millis
operation_timeout_millis
```

`connect_timeout_millis` 覆盖：

```text
ProxyCommand spawn
+
stream establishment
+
SSH connect / handshake
```

`operation_timeout_millis` 覆盖：

```text
Authentication
SFTP
Remote metadata
read_range
```

只有实际基准证明有必要时，后续才考虑：

```text
proxy_start_timeout_millis
proxy_shutdown_timeout_millis
```

## 19. Cache / Sync 不变化

ProxyCommand 不能影响 Remote Source 查询模型。

仍然：

```text
Remote Server
     │
     ▼
SSH/SFTP
     │
     ▼
Sync Engine
     │
     ▼
Generation Cache
     │
     ▼
Snapshot
     │
     ▼
Local Scanner
     │
     ▼
Query Engine
```

禁止演化为：

```text
ProxyCommand
↓
remote grep
↓
return result
```

因此现有：

```text
full
tail
from_now
incremental append
rotation
truncate
continuity fingerprint
CACHE_SCOPE_EXCEEDED
```

语义全部保持不变。

## 20. 配置兼容性

由于 v2 尚未正式发布，建议直接扩展现有 v2 Schema。

老配置继续表示 Direct，新配置增加 `proxy.type=command` 表示 ProxyCommand，因此不需要 `version = 3`。

## 21. JSON Schema 建议

新增 `ProxyCommandConfig`：

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "type",
    "program",
    "args"
  ],
  "properties": {
    "type": {
      "const": "command"
    },
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
```

然后给 `SshConnectionConfig.properties.proxy` 增加：

```json
{
  "$ref": "#/$defs/ProxyCommandConfig"
}
```

并继续保持 `additionalProperties = false`。

## 22. Runtime Validation

启动时增加以下验证：

1. `proxy.type` 必须是 `command`。
2. `program` 不得为空。
3. args 数量不得超过硬限制。
4. 单个 arg 长度不得超过硬限制。
5. 仅允许已定义 Placeholder。
6. `{host}`、`{port}` 来自 Connection Config，而不是 MCP 请求。
7. Credential 不允许作为 Placeholder。
8. 不解析 Shell syntax。
9. 不执行 `$VAR`、`${VAR}`、`%VAR%` 等自定义模板语义。
10. ProxyCommand 配置不得改变 Host Key Verification。
11. Connection 仍然必须配置 `host` 和 `port`。
12. Connection 仍然必须配置正常 SSH Authentication。

## 23. 测试计划

### M7-A Config

测试：

```text
direct config PASS
proxy config PASS
unknown proxy type rejected
missing program rejected
too many args rejected
unknown placeholder rejected
unknown fields rejected
```

### M7-B Direct Regression

现有 Direct SSH 全部重新运行：

```text
password auth
private key auth
encrypted private key
known_hosts
SFTP
range read
timeout
disconnect
server restart
multi-server
```

必须保证：增加 ProxyCommand 后 Direct Transport 无行为变化。

### M7-C Proxy Integration

建立 Test Proxy：

```text
Log Query MCP
   │
   ▼
test proxy process
   │
   ▼
OpenSSH test server
```

测试：

```text
SSH handshake
password auth
private key auth
known_hosts
SFTP stat
SFTP lstat
read_dir
read_range
```

## 24. Failure Matrix

至少覆盖：

```text
program 不存在
program 无执行权限
spawn 失败
proxy early exit
proxy stdout EOF
proxy stderr 大量输出
connect timeout
SSH handshake failure
host key mismatch
authentication failure
SFTP failure
network disconnect
query cancellation
server restart
proxy process crash
```

所有情况下都必须 fail-closed，并确保：

```text
no orphan process
no leaked SFTP handle
no leaked SSH permit
no secret leakage
```

## 25. WSL Acceptance

增加一个真实或半真实 WSL Acceptance Scenario。

目标：

```text
WSL direct path unavailable
Windows host path available
```

验证：

```text
log-query-mcp in WSL
↓
Windows ProxyCommand
↓
SSH server
↓
SFTP
↓
Sync
↓
Cache
↓
search_logs
```

必须能够完成：

```text
list_log_sources
search_logs
get_log_context
```

## 26. Multi-Server

ProxyCommand 必须支持不同 Server 使用不同 Transport。

例如：

```text
test-01
 └── direct

test-02
 └── Windows ProxyCommand

prod-01
 └── ProxyCommand A

prod-02
 └── ProxyCommand B
```

其中一个 ProxyCommand 失败不得影响其他 Connection、Local Source 或其他 Remote Source。

## 27. Performance

ProxyCommand 增加一层 Process IO 后，需要重新记录：

```text
connection setup latency
full bootstrap throughput
tail bootstrap throughput
incremental sync throughput
single-server concurrency
dual-server concurrency
```

不要求 ProxyCommand 与 Direct 完全相同。

核心验收标准是：

```text
无明显异常退化
无 deadlock
无 process leak
无无限 buffering
```

大文件测试至少复用：

```text
100 MiB full
1 GiB full
10 GiB logical tail
```

## 28. 可观测性

建议增加内部指标：

```text
ssh_transport=direct|proxy_command
proxy_spawn_duration
ssh_handshake_duration
proxy_exit_code
proxy_failure_category
```

日志可以记录：

```text
connection_id=test-01
transport=proxy_command
```

但不能记录 credential、secret value、完整敏感 argv、ProxyCommand stdout 或无限 stderr。

## 29. 实施阶段

建议定义为 M7。

### M7-0 Design / Contract

```text
PROXY_COMMAND_TRANSPORT_V2.md
CONFIG_SCHEMA_V2.md
JSON Schema
ADR
TODO
```

### M7-1 Stream Abstraction

抽取：

```text
SshStreamConnector
DirectConnector
```

确保 Direct Regression 全绿。

### M7-2 ProxyCommand Connector

实现：

```text
process spawn
stdin/stdout stream
placeholder expansion
stream-based SSH connect
```

### M7-3 Lifecycle / Security

实现：

```text
timeout
cancellation
kill/wait
stderr bound
redaction
failure mapping
```

### M7-4 Integration

实现：

```text
SSH
SFTP
Sync
Cache
Query
Multi-server
```

完整测试。

### M7-5 WSL Acceptance

验证：

```text
WSL
→ Windows host command
→ remote SSH
```

### M7-6 Final Gate

重新执行：

```text
Rust
Contracts
SSH Transport
M6/M7 Security
Performance
Release
```

## 30. ADR 建议

新增：

```text
ADR-0011-use-proxy-command-as-ssh-stream-transport.md
```

Decision：

> Log Query MCP 支持管理员配置的 ProxyCommand 作为 SSH 底层字节流 Transport，但不向 AI 暴露命令执行能力，也不允许 ProxyCommand 执行结果绕过 SSH/SFTP、Cache 和 Query Engine。

Positive：

```text
支持 WSL / Windows VPN
支持隔离网络
支持堡垒网络
支持自定义 TCP Transport
保留 SSH/SFTP 安全模型
不扩大 MCP API
不引入通用 Shell 能力
```

Negative：

```text
增加本地进程生命周期管理
增加平台兼容测试
需要处理 cancellation / child cleanup
需要重新执行 Transport 性能基线
```

## 31. 最终安全边界

增加 ProxyCommand 后，系统仍然保持：

```text
AI
 │
 │ 只能调用日志工具
 ▼
Log Query MCP
 │
 │ 管理员静态配置
 ▼
Connection
 │
 ├── Direct
 │
 └── ProxyCommand
        │
        │ 只提供 raw byte stream
        ▼
       SSH
        │
        ▼
       SFTP
        │
        ▼
管理员授权的日志文件
```

明确禁止变成：

```text
AI
 │
 ▼
ProxyCommand
 │
 ▼
Shell
```

这是整个功能最重要的设计边界。

## 32. Acceptance Criteria

ProxyCommand M7 完成的定义：

- [ ] 原有 Direct SSH 行为完全兼容。
- [ ] `proxy.type=command` 配置可建立 SSH Connection。
- [ ] ProxyCommand stdout 作为 SSH raw byte stream。
- [ ] ProxyCommand 不经过 Shell 字符串解析。
- [ ] AI 无法控制 program / args / host / port。
- [ ] Password / Private Key 仍由原 SSH Authentication 管理。
- [ ] Strict Host Key Verification 保持启用。
- [ ] ProxyCommand 无法绕过 SFTP-only Remote Access。
- [ ] ProxyCommand 失败时 fail-closed。
- [ ] Timeout / Cancellation 后不存在 orphan process。
- [ ] 不泄露 credential / raw stderr / sensitive argv。
- [ ] Remote Sync / Cache / Snapshot / Query 语义不变化。
- [ ] Local + Direct Remote + Proxy Remote 可以同时查询。
- [ ] 单服务器失败不影响其他服务器。
- [ ] WSL → Windows Host → Remote SSH 场景验收通过。
- [ ] Direct / ProxyCommand SSH live tests PASS。
- [ ] Security / Fault Matrix PASS。
- [ ] Large-file / concurrency regression PASS。
- [ ] Final RC Gate 重新执行完成。

## 33. 最终结论

ProxyCommand 应当作为：

```text
SSH Transport capability
```

而不是：

```text
Command Execution capability
```

其唯一职责：

```text
program stdin/stdout
        ↓
SSH raw stream
```

整个系统仍然坚持：

```text
SSH = Transport
SFTP = Remote Read
Cache = Stable Local Snapshot
Query Engine = Search
MCP = AI-facing API
```

因此 ProxyCommand 能解决：

```text
WSL 无法直连
Windows Host 可以访问
VPN / 特殊路由
堡垒网络
自定义 TCP Relay
```

同时不会破坏 Log Query MCP 原有的：

```text
单一职责
关注分离
只读安全边界
AI 最小权限
```
