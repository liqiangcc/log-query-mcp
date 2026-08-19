# Log Query MCP v2：SSH 远程日志同步与本地查询方案

> 状态：方案草案  
> 目标版本：v2  
> 核心目标：MCP 本地部署，通过 SSH/SFTP 访问远程服务器，将日志安全、增量地同步到本地缓存，再复用现有查询引擎进行搜索。

---

## 1. 背景

当前 Log Query MCP 的部署模型为：

```text
AI / MCP Client
      │
      ▼
Log Query MCP
      │
      ▼
服务器本地日志
```

这种方案具有很强的本地文件安全边界，但存在明显的部署成本：

1. 每台服务器都需要安装和维护 MCP。
2. 多台开发、测试服务器需要分别部署、升级和配置。
3. 部分服务器不适合额外安装长期运行的服务。
4. AI 客户端实际上只需要获得受控日志，并不一定需要 MCP 与日志部署在同一台服务器。
5. SSH 通常已经是服务器现成的管理通道。

因此 v2 引入新的运行模式：

```text
AI
 │
 ▼
本地 Log Query MCP
 │
 ├── 本地日志
 │
 └── SSH / SFTP
          │
          ▼
       远程服务器
          │
          ▼
       日志文件
```

远程日志不直接通过 Shell 搜索，而是通过 SFTP 安全读取并同步到本地缓存，由本地查询引擎统一完成搜索。

---

## 2. 核心目标

### 2.1 MCP 可以完全本地部署

远程服务器只需要：

```text
SSH Server
+
SFTP
+
只读日志账号
```

不需要：

```text
安装 MCP
安装 Agent
安装额外守护进程
开放 MCP HTTP 端口
```

### 2.2 支持多服务器

一个本地 MCP 可以配置：

```text
Server A
Server B
Server C
...
```

例如：

```text
本地 log-query-mcp
 │
 ├── test-01
 │    ├── order-service
 │    └── payment-service
 │
 ├── test-02
 │    └── user-service
 │
 └── dev-01
      └── gateway
```

AI 无需关心日志位于本机还是远程服务器。

### 2.3 保持 MCP API 简单

v2 默认仍然只向 AI 暴露：

```text
list_log_sources
search_logs
get_log_context
```

不增加：

```text
ssh_exec
run_shell
read_remote_file
download_file
sync_log
grep_remote
```

SSH、同步、缓存全部属于 MCP 内部实现细节。

---

## 3. 非目标

v2 不建设通用 SSH MCP。

明确不提供：

- 任意 Shell 命令执行。
- 任意服务器文件读取。
- 文件上传。
- 文件修改。
- 文件删除。
- sudo。
- 服务重启。
- 部署应用。
- SSH 终端。
- 任意 SCP。
- 客户端指定服务器文件路径。

Log Query MCP 的边界仍然是：

> **只读取管理员预先授权的日志来源。**

部署、Shell、文件上传等能力应属于独立的 Deployment/SSH MCP，不应混入日志查询 MCP。

---

## 4. 核心设计原则

### 4.1 SSH 只作为 Transport

SSH 的职责：

```text
建立安全连接
认证
服务器身份验证
SFTP 文件读取
```

SSH 不负责：

```text
搜索
grep
日志分析
Shell 执行
```

### 4.2 Cache 作为本地日志副本

远程日志经过：

```text
SSH/SFTP
   ↓
Sync Engine
   ↓
Local Cache
```

然后查询：

```text
Local Cache
   ↓
现有 Scanner
   ↓
现有 Query Engine
```

这样本地日志与远程日志最终都进入统一查询模型。

### 4.3 查询优先使用本地数据

`search_logs` 的逻辑不是：

```text
查询
↓
SSH grep
↓
返回结果
```

而是：

```text
查询
 ↓
检查缓存新鲜度
 ↓
必要时增量同步
 ↓
生成稳定本地快照
 ↓
本地搜索
 ↓
返回结果
```

### 4.4 不允许静默查询过期缓存

如果配置要求查询前刷新，而远程服务器不可达，不能直接搜索旧缓存然后返回“没有找到日志”，否则会制造假阴性。

正确行为：

```text
SSH 同步失败
↓
明确返回 SOURCE_UNAVAILABLE / REMOTE_UNAVAILABLE
```

后续可以增加显式配置：

```text
allow_stale_on_error
```

但默认应关闭。

---

## 5. 总体架构

```text
                     MCP Client / AI
                            │
                            ▼
                    ┌───────────────┐
                    │  MCP API      │
                    │               │
                    │ list_sources  │
                    │ search_logs   │
                    │ get_context   │
                    └───────┬───────┘
                            │
                            ▼
                    ┌───────────────┐
                    │ SourceManager │
                    └───────┬───────┘
                            │
               ┌────────────┴────────────┐
               │                         │
               ▼                         ▼
        ┌─────────────┐           ┌─────────────┐
        │ LocalSource │           │ RemoteSource│
        └──────┬──────┘           └──────┬──────┘
               │                         │
               │                         ▼
               │                  ┌──────────────┐
               │                  │ SSH/SFTP     │
               │                  │ Transport    │
               │                  └──────┬───────┘
               │                         │
               │                         ▼
               │                  Remote Server
               │                         │
               │                         ▼
               │                    Log Files
               │                         │
               │                         ▼
               │                  ┌──────────────┐
               │                  │ Sync Engine  │
               │                  └──────┬───────┘
               │                         │
               │                         ▼
               │                  ┌──────────────┐
               └─────────────────►│ Local Cache  │
                                  └──────┬───────┘
                                         │
                                         ▼
                                  Existing Scanner
                                         │
                                         ▼
                                  Query Engine
```

---

## 6. Source Backend 抽象

建议把文件来源抽象为：

```text
LogSourceBackend
```

至少实现：

```text
LocalBackend
RemoteSshBackend
```

但不要让查询引擎直接操作 SSH。

更推荐分层：

```text
RemoteSshBackend
       │
       ▼
SyncEngine
       │
       ▼
LocalCache
       │
       ▼
QueryEngine
```

也就是说 QueryEngine 最终面对的仍然是本地稳定文件。

这样可以最大程度复用 v1 搜索代码。

未来还可以自然增加：

```text
KubernetesBackend
S3Backend
LokiBackend
```

而无需修改 MCP 工具接口。

---

## 7. SSH Connection 配置

建议配置升级到 `version = 2`，引入独立 `connections`。

示例：

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
        "known_hosts_file": "~/.ssh/known_hosts"
      }
    }
  ]
}
```

---

## 8. SSH 认证

至少支持 Password 和 SSH Key 两种认证。

### 8.1 Password

支持用户名密码，但密码不能直接写入普通配置文件。

错误：

```json
{
  "username": "root",
  "password": "123456"
}
```

推荐：

```json
{
  "username": "log-reader",
  "auth": {
    "type": "password",
    "secret_ref": "TEST_SERVER_PASSWORD"
  }
}
```

SecretResolver 第一阶段可以支持环境变量，未来扩展：

```text
OS Keychain
Vault
1Password
AWS Secrets Manager
其他 Secret Manager
```

### 8.2 SSH Key

支持：

```json
{
  "auth": {
    "type": "private_key",
    "key_file": "~/.ssh/log-reader"
  }
}
```

后续可增加 SSH Agent。

推荐认证优先级：

```text
SSH Agent
   >
Private Key
   >
Password
```

但 Password 必须作为正式支持能力，因为大量测试环境仍然使用用户名密码认证。

---

## 9. Host Key Verification

服务器身份认证必须默认开启。

禁止默认使用：

```text
StrictHostKeyChecking=no
```

或者任何等效的 `accept all host keys`。

应使用 `known_hosts`。

发生 host key 改变时连接立即失败，不能静默接受。

---

## 10. 服务器账号安全模型

Remote 模式无法直接复用本机 Linux `openat2()` 的全部安全保证。

因此远程模式的真正安全边界需要调整为：

```text
MCP 配置
+
SSH 账号权限
+
SFTP 服务端限制
```

强烈推荐创建专门用户：

```text
log-reader
```

该用户应该：

```text
只读
无 sudo
不能修改日志
不能访问无关目录
```

生产场景最好进一步限制为：

```text
SFTP Only
+
Chroot
```

例如：

```text
log-reader
       │
       ▼
允许访问 /logs
       │
       ├── application.log
       ├── application.log.1
       └── archive/
```

这样即使 MCP 发生路径处理漏洞，该账号本身也无法越过服务器权限边界。

---

## 11. Source 配置

日志 Source 保持现有概念，但增加 backend。

本机日志：

```json
{
  "source_id": "local-payment",
  "backend": {
    "type": "local"
  },
  "root": "/var/log/payment",
  "files": [
    "application.log"
  ]
}
```

远程日志：

```json
{
  "source_id": "remote-order-test",
  "backend": {
    "type": "ssh",
    "connection_id": "test-server-01"
  },
  "root": "/data/log/order-service",
  "files": [
    "application.log",
    "application.log.1"
  ]
}
```

目录规则继续支持：

```json
{
  "directories": [
    {
      "path": "archive",
      "recursive": false,
      "include_suffixes": [
        ".log",
        ".log.1"
      ]
    }
  ]
}
```

AI 依然只能看到：

```text
source_id
name
description
service
environment
tags
```

不能看到：

```text
host
port
username
password
secret_ref
root
绝对路径
```

---

## 12. 本地 Cache

建议统一缓存目录：

```text
~/.cache/log-query-mcp/
```

逻辑结构：

```text
cache/
└── sources/
    └── order-test/
        ├── manifest.json
        │
        └── files/
            ├── application.log/
            │   ├── generation-001.log
            │   └── generation-002.log
            │
            └── application.log.1/
                └── generation-001.log
```

缓存路径必须由 MCP 自己生成，不能根据客户端输入直接形成文件路径。

---

## 13. Cache Manifest

每个远程文件维护元数据。

例如：

```json
{
  "source_id": "order-test",
  "file_id": "file_xxx",
  "remote_relative_path": "application.log",
  "generation": "generation-002",
  "remote_size": 838860800,
  "cached_size": 838860800,
  "remote_mtime": 1786100000,
  "last_sync_at": "2026-08-07T15:20:00+08:00",
  "continuity_fingerprint": "..."
}
```

manifest 必须原子更新：

```text
write temp
↓
validate
↓
rename
```

进程崩溃不能破坏最后一次有效状态。

---

## 14. 同步模型

同步分成：

```text
Bootstrap
+
Incremental Sync
```

---

## 15. Bootstrap

第一次访问远程日志时可能面对非常大的日志文件，因此不能强制默认全量下载。

需要显式定义 Bootstrap 策略。

### 15.1 full

完整同步：

```text
0 ----------------------------- EOF
```

优点：历史查询完整。

缺点：首次同步慢，占用磁盘大。

### 15.2 tail

只同步最近 N Bytes：

```text
0 --------------------|--------- EOF
                     ↑
                  从这里同步
```

适合问题排查。

### 15.3 from_now

启动时只记录当前位置：

```text
历史日志不下载

从 MCP 启动之后开始增量同步
```

适合只关心未来日志的环境。

### 15.4 查询完整性

如果本地缓存只覆盖最近一部分日志，而用户查询的时间范围可能超出缓存范围，系统不能返回空 `results` 让 AI 误认为服务器没有该日志。

应该明确返回：

```text
CACHE_SCOPE_EXCEEDED
```

或其他等价的“数据覆盖不完整”错误。

原则：

> **宁可告诉 AI 数据不完整，也不能制造假阴性。**

---

## 16. 增量同步

正常情况下：

```text
remote size = 100MB
cached size = 100MB
```

之后：

```text
remote size = 105MB
```

只下载：

```text
100MB → 105MB
```

而不是重新下载整个文件。

流程：

```text
Remote stat
     │
     ▼
比较 manifest
     │
     ├── unchanged
     │       ↓
     │     不同步
     │
     └── appended
             ↓
       从 cached_offset 读取
             ↓
         临时文件
             ↓
       连续性验证
             ↓
         合并缓存
             ↓
       更新 manifest
```

---

## 17. 防止错误增量

仅比较 `size` 和 `mtime` 不够可靠。

文件可能被替换、覆盖，或 truncate 后重新增长。

因此需要维护：

```text
continuity fingerprint
```

例如记录缓存末尾附近的一小段内容 Hash。

下次增量之前：

```text
读取远程旧 offset 附近数据
↓
计算 Hash
↓
与本地记录比较
```

一致：继续 append。

不一致：创建新 generation。

这样比单纯依赖 mtime/size 更可靠。

---

## 18. 日志轮转

必须处理：

```text
application.log
↓
application.log.1

新的 application.log
```

典型变化：

```text
之前：

application.log
size = 2GB

之后：

application.log
size = 20MB

application.log.1
size = 2GB
```

此时不能继续按照旧 offset 读取。

应该：

```text
Remote Metadata
      │
      ▼
发现 truncate / replacement
      │
      ▼
关闭旧 generation
      │
      ▼
建立新 generation
```

例如：

```text
application.log/
├── generation-001.log
└── generation-002.log
```

旧 generation 不立即删除。

---

## 19. 为什么需要 Generation

现有 `match_ref`、`cursor` 可能仍然引用旧日志。

如果远程文件轮转后立即覆盖本地缓存，会导致旧 `match_ref` 定位错误。

因此：

```text
search_logs
↓
绑定 generation-001
```

即使之后远程文件轮转并生成 `generation-002`，之前生成的 cursor 和 match_ref 仍然可以短时间安全访问 `generation-001`，直到 token TTL 到期，然后 Cache GC 才可以清理。

---

## 20. 查询一致性

每次查询建立一个本地 Snapshot：

```text
Source
+
File Generation
+
File Length
```

例如：

```text
application.log
generation = 17
snapshot_size = 812345678
```

查询开始之后即使又同步了新日志，当前 cursor 仍然只查询原 snapshot 范围，下一次新查询才能看到新增数据。

这样可以保证分页结果稳定。

---

## 21. search_logs 执行流程

完整流程：

```text
search_logs
    │
    ▼
Validate Request
    │
    ▼
Resolve Source
    │
    ├──────── Local
    │           │
    │           ▼
    │       Local Snapshot
    │
    └──────── Remote
                │
                ▼
          Ensure Fresh
                │
                ▼
           SSH/SFTP
                │
                ▼
          Incremental Sync
                │
                ▼
           Cache Snapshot
                │
                ▼
          Existing Scanner
                │
                ▼
           Query Engine
                │
                ▼
             Result
```

从 Scanner 开始应该尽量复用现有实现。

---

## 22. get_log_context

`match_ref` 不应该记录远程服务器路径，而应该绑定：

```text
source_id
file_id
cache_generation
local snapshot
line/offset
```

然后：

```text
get_log_context
↓
读取对应本地 generation
```

因此调用 `get_log_context` 通常不需要再次建立 SSH 连接。

这能够明显降低延迟、SSH 请求次数和服务器负载。

---

## 23. SSH Connection Pool

多个查询不应该每次都重新 connect/authenticate/disconnect。

建议维护有限连接池：

```text
ConnectionManager
      │
      ├── Server A
      │      └── SSH Connection
      │
      └── Server B
             └── SSH Connection
```

需要支持：

```text
connect timeout
operation timeout
keepalive
idle timeout
max connections
```

同时必须有全局并发限制，避免 AI 一次请求导致大量服务器连接。

---

## 24. Cache 资源限制

必须配置：

```text
max_cache_bytes
max_cache_bytes_per_source
retention
max_generations_per_file
```

例如概念配置：

```json
{
  "cache": {
    "root": "~/.cache/log-query-mcp",
    "max_bytes": 21474836480,
    "retention_hours": 168,
    "max_generations_per_file": 4
  }
}
```

具体默认值应该经过性能测试之后冻结，不应在设计阶段随意确定。

---

## 25. Cache GC

清理条件：

```text
已经过期
且
没有 match_ref 引用
且
没有 cursor 引用
```

然后按照最旧访问时间执行淘汰。

不能删除当前 Query Snapshot、当前 match_ref、当前 cursor 正在使用的 generation。

---

## 26. 本地缓存权限

缓存可能包含生产日志。

因此缓存目录至少：

```text
directory = 0700
file      = 0600
```

不得 world-readable。

缓存中禁止保存：

```text
password
private key
secret
```

认证信息和日志缓存必须完全分离。

---

## 27. Freshness Policy

建议 Remote Source 首先支持：

```text
on_query
```

即每次查询检查远程变化，有变化才同步。

这应该作为 v2 第一阶段主要策略。

后续可以增加：

```text
background
```

例如每 N 秒同步，用于频繁查询的测试环境。

但 Background Sync 不应成为 v2 MVP 的必要条件。

---

## 28. Remote Directory Discovery

AI 仍然不能提交 glob。

管理员继续配置目录规则：

```json
{
  "path": "archive",
  "recursive": false,
  "include_suffixes": [
    ".log"
  ]
}
```

Remote SourceManager 通过 SFTP：

```text
list directory
↓
过滤 suffix
↓
限制数量
↓
生成稳定 file_id
```

所有数量仍然受资源限制。

---

## 29. 路径安全

Remote 模式不能假设 `realpath prefix check` 就是可靠安全边界。

需要至少：

1. 配置路径规范化。
2. 禁止 `..`。
3. 禁止绝对子路径。
4. 不接受客户端路径。
5. 尽可能使用 SFTP `lstat` 检查软链接。
6. Dedicated SSH account。
7. 推荐 SFTP chroot。

最终安全原则：

```text
MCP 路径限制
+
SSH Unix Permission
+
SFTP Chroot
```

形成纵深防御。

---

## 30. 错误模型

建议 v2 新增或明确以下错误：

```text
REMOTE_UNAVAILABLE
REMOTE_AUTH_FAILED
HOST_KEY_VERIFICATION_FAILED

REMOTE_FILE_CHANGED

SYNC_FAILED
CACHE_SCOPE_EXCEEDED
CACHE_LIMIT_EXCEEDED
CACHE_CORRUPTED
```

错误消息不得包含：

```text
password
private key
secret_ref 内容
远程绝对敏感路径
Rust backtrace
底层 SSH 凭证
```

---

## 31. 查询超时拆分

Remote 查询增加两个阶段：

```text
Sync
+
Search
```

因此需要分别控制：

```text
connect_timeout
sync_timeout
query_timeout
```

否则一次服务器网络异常可能长时间占住 MCP 请求。

---

## 32. 可观测性

本地 MCP 日志可以记录：

```text
connection_id
source_id
sync duration
downloaded bytes
cache hit
cache miss
cache size
remote file changed
rotation detected
query duration
```

不得记录：

```text
SSH password
private key
完整搜索关键字
完整日志内容
```

---

## 33. MCP API 兼容策略

v2 最重要的原则：

> **尽可能不修改 MCP 工具 API。**

仍然：

```text
list_log_sources
search_logs
get_log_context
```

AI 不需要学习 SSH、Cache、Sync、Generation，这些都属于服务内部实现。

因此：

```text
AI → Log Source
```

而不是：

```text
AI → Server → SSH → File
```

---

## 34. 配置兼容性

v1：

```text
version = 1
```

继续按照现有 Local 模式运行。

v2：

```text
version = 2
```

增加：

```text
connections
backend
cache
remote-specific limits
```

不应该改变 `version=1` 的现有安全语义。

这样旧配置仍可以继续正常运行，而不是升级程序以后必须重写配置。

---

## 35. 推荐代码结构

基于当前实现，建议逐渐演进为：

```text
src/

├── backend/
│   ├── mod.rs
│   ├── local.rs
│   └── remote.rs
│
├── transport/
│   ├── mod.rs
│   └── ssh.rs
│
├── cache/
│   ├── mod.rs
│   ├── store.rs
│   ├── manifest.rs
│   ├── generation.rs
│   ├── sync.rs
│   └── gc.rs
│
├── config.rs
│
├── source_discovery.rs
│
├── scanner.rs
├── scan_executor.rs
│
├── query_engine.rs
├── query_state.rs
│
├── context_reader.rs
├── context_executor.rs
│
├── mcp_model.rs
└── mcp_server.rs
```

其中：

```text
scanner
query_engine
context_reader
```

尽量不感知 SSH。

---

## 36. 核心模块职责

### ConnectionManager

负责：

```text
SSH 建连
认证
Host Key 验证
连接复用
超时
Keepalive
```

### RemoteSource

负责：

```text
远程文件发现
远程 metadata
远程文件标识
```

### SyncEngine

负责：

```text
Bootstrap
Incremental Sync
Rotation Detection
Continuity Verification
```

### CacheStore

负责：

```text
本地文件
Manifest
Generation
Atomic Commit
Quota
GC
```

### QueryEngine

继续负责：

```text
字面量搜索
时间过滤
排序
分页
```

QueryEngine 不应该知道：

```text
SSH password
host
remote path
SFTP
```

---

## 37. 典型使用场景

### 场景一：测试环境排查 Bug

开发人员配置：

```text
test-server
order-service
```

AI：

```text
读取本地代码
↓
search_logs(traceId)
```

MCP：

```text
SSH 连接服务器
↓
发现 application.log 新增 3MB
↓
下载 3MB
↓
更新本地缓存
↓
本地搜索
↓
返回异常
```

AI 再调用 `get_log_context` 时，直接读取本地缓存，不需要 SSH。

### 场景二：多服务 Trace

例如：

```text
gateway
order
payment
inventory
```

分别位于不同服务器。

AI 请求：

```text
source_ids = [
  gateway-test,
  order-test,
  payment-test,
  inventory-test
]
```

MCP：

```text
并发受控地刷新多个 Remote Source
↓
构建本地 Snapshot
↓
统一查询
↓
按照日志时间排序
```

AI 无需人工登录多台服务器。

---

## 38. 性能目标

### 38.1 已缓存日志

查询性能尽量接近当前 Local 模式：

```text
SSH = 0
Network = 0
```

### 38.2 增量日志

网络传输量大致等于新增日志量，而不是整个日志大小。

### 38.3 get_log_context

正常情况下：

```text
0 次远程请求
```

直接读取当前 cache generation。

---

## 39. 安全目标

Remote 模式必须满足：

```text
AI 不能执行 Shell
AI 不能提交路径
AI 看不到 SSH 凭证
AI 不能访问未配置日志
MCP 不能修改服务器日志
```

即使支持用户名 + 密码，也不能演变成通用 SSH 权限。

---

## 40. v2 MVP 范围

第一阶段建议只做：

```text
SSH/SFTP
+
Password authentication
+
Private key authentication
+
Host key verification
+
Remote explicit files
+
Remote directory discovery
+
Local cache
+
On-query sync
+
Incremental append
+
Rotation detection
+
Cache generation
+
Cache quota
```

继续复用：

```text
search_logs
get_log_context
cursor
match_ref
scanner
query engine
```

---

## 41. v2 MVP 暂不实现

第一阶段不做：

```text
SSH Shell Exec
Remote grep

Deployment
File Upload

Kubernetes
Docker API

Loki
Elasticsearch

日志实时 follow

全文索引

分布式缓存

多实例共享缓存

复杂 Secret Manager 集成

自动后台长周期同步
```

避免范围迅速膨胀。

---

## 42. 实施计划

### M0：ADR 和契约冻结

首先确定：

```text
Remote Source Architecture
SSH Security Boundary
Cache Consistency Model
Generation Model
Bootstrap Semantics
```

新增 ADR。

同时定义：

```text
config-v2.schema.json
error-v2.schema.json
```

这个阶段不写 SSH 主逻辑。

### M1：Source Backend 抽象

目标：先让 Local 文件读取通过新的 backend 抽象运行。

确保所有 v1 测试继续通过。

这一阶段完成以后，架构才允许添加 RemoteBackend。

### M2：SSH Transport

实现：

```text
SSH connection
password auth
key auth
known_hosts
SFTP
timeout
connection manager
```

只测试远程：

```text
stat
list
read range
```

不接 QueryEngine。

### M3：Cache Store

实现：

```text
cache directory
manifest
generation
atomic metadata
quota
GC
```

### M4：Sync Engine

实现：

```text
bootstrap
append sync
continuity check
truncate
replacement
rotation
network interruption recovery
```

### M5：Query Integration

打通：

```text
Remote Source
↓
ensure_fresh
↓
snapshot
↓
scanner
↓
query engine
```

然后让：

```text
search_logs
get_log_context
cursor
match_ref
```

全部支持 Remote Source。

### M6：安全和故障测试

重点测试：

```text
Wrong Password
Wrong Host Key
SSH Disconnect
SFTP Disconnect
Server Restart
File Append
File Rotation
File Truncate
File Replacement
Cache Full
Local Disk Full
MCP Crash During Sync
Concurrent Search
Multiple Servers
```

---

## 43. 关键验收标准

### 43.1 功能

- 一台远程服务器无需安装 MCP 即可查询日志。
- 一个 MCP 可以管理多台 SSH Server。
- 支持用户名密码。
- 支持 SSH Key。
- 支持增量同步。
- 支持日志轮转。
- 本地缓存可持久化。
- MCP 重启后缓存仍可恢复。

### 43.2 查询

同一份日志通过：

```text
LocalBackend
```

与：

```text
RemoteBackend → Cache
```

查询结果语义一致。

### 43.3 安全

AI 无法：

```text
执行 Shell
读取任意路径
获取 SSH 密码
上传文件
修改日志
```

错误 host key 必须拒绝连接。

### 43.4 正确性

缓存不完整时，不得返回假阴性的空结果。

同步失败时，不得静默使用过期缓存。

### 43.5 稳定性

SSH 中断不能损坏已有缓存。

同步中进程崩溃后，重启可以恢复。

日志轮转时，已有 match_ref/cursor 在 TTL 范围内仍能正确定位旧 generation。

---

## 44. 最终架构结论

v2 不应该把 Log Query MCP 变成：

```text
SSH MCP
+
grep
```

而应该形成：

```text
                  Log Query MCP
                        │
        ┌───────────────┼──────────────┐
        │               │              │
    Transport        Cache          Search
        │               │              │
    SSH/SFTP        Local Disk     Existing Engine
        │
        ▼
  Remote Servers
```

即：

```text
SSH = 数据传输层
Cache = 本地数据层
Query Engine = 查询层
MCP API = AI 能力层
```

服务器只承担：

```text
存放日志
+
SFTP 只读访问
```

本地 MCP 承担：

```text
连接管理
日志同步
缓存管理
日志搜索
分页
上下文
安全限制
```

最终使用体验：

```text
用户只安装一个 log-query-mcp
              │
              ├── server-a
              ├── server-b
              ├── server-c
              └── server-n
```

这是 v2 最值得采用的核心架构。

---

## 45. 推荐最终决策

**D1**  
Log Query MCP 同时支持：

```text
Local Source
Remote SSH Source
```

**D2**  
Remote Source 只允许：

```text
SSH + SFTP
```

不允许：

```text
SSH Exec
```

**D3**  
远程日志先同步/缓存，再本地查询，而不是远程执行搜索命令。

**D4**  
默认采用：

```text
On-query Incremental Sync
```

后续再增加后台预同步。

**D5**  
日志同步必须增量、有界、可恢复、支持 Rotation，不能简单每次完整下载。

**D6**  
SSH Password 正式支持，但密码通过 SecretResolver 获取，不以明文长期保存在普通配置中。

**D7**  
Host Key Verification 默认强制开启。

**D8**  
Remote 模式的服务器安全边界由专用只读 SSH 用户、服务器文件权限、推荐 SFTP Chroot 共同提供。

**D9**  
MCP 工具层继续保持：

```text
list_log_sources
search_logs
get_log_context
```

SSH 和 Cache 对 AI 完全透明。

**D10**  
v1 不修改原有语义；Remote Source 使用新的 v2 配置契约。

---

## 46. 一句话定义 v2

> **Log Query MCP v2 是一个本地运行、通过安全远程传输获取受控日志、通过增量本地缓存降低网络和服务器开销，并统一使用本地查询引擎为 AI 提供只读日志检索能力的 MCP 服务。**
