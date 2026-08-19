# Log Query MCP v2 M2 实现基线

> 状态：M2 implementation complete  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`  
> M2 代码 Gate 基线：`2e4110ba2c6ebe1cff5d2c9d1dab2a9aa57046ce`  
> 上游清单：[`REMOTE_SSH_CACHE_TODO_V2.md`](./REMOTE_SSH_CACHE_TODO_V2.md)

## 1. M2 目标

M2 只解决两件事：

1. 在配置中只保存凭据引用，并在真正建立 SSH 会话时解析 Secret。
2. 建立严格只读、无远程命令执行能力的 SSH/SFTP Transport。

M2 **不实现 CacheStore、SyncEngine、Remote Source 查询集成，也不把 SSH Source 接入现有 Query Engine**。

这些职责分别属于 M3、M4、M5。

---

## 2. SecretResolver

新增：

```text
SecretResolver
└── EnvSecretResolver
```

当前 MVP 只支持环境变量形式的 Secret Reference。

原则：

```text
配置文件只保存 secret_ref
        ↓
建立 SSH 会话时按需解析
        ↓
SecretValue
        ↓
认证结束后不进入日志、Debug、Tool Error
```

### 2.1 Secret 安全边界

`SecretValue` 的 Debug 输出固定为：

```text
SecretValue(<redacted>)
```

不会打印真实 Secret。

Secret Reference 只允许受控的环境变量标识符：

```text
[A-Z_][A-Z0-9_]*
```

并设置长度上限。

当前不会从 MCP 请求中接收密码，也不会把普通 `password` 字段加入配置 Schema。

---

## 3. SSH Connection Manager

新增：

```text
SshConnectionManager
        ↓
OwnedSemaphorePermit
        ↓
独立 SSH Session
        ↓
独立 SFTP Session
        ↓
SshReadTransport
```

### 3.1 并发模型

M2 **没有实现 SSH 连接池**。

每个 Reader 建立一个独立 SSH/SFTP Session，全局只通过：

```text
max_concurrent_ssh_connections
```

控制并发连接数量。

这样可以直接保证：

- Broken Session 不可能重新进入连接池。
- 不存在 Idle Session 清理问题。
- Reader Drop 时自动释放 Semaphore Permit。
- Connect Future 被取消时 Permit 自动释放。
- 后续如果性能数据证明需要连接池，再独立实现和验证连接池生命周期。

M2 优先选择可证明正确的生命周期，而不是提前引入连接复用复杂度。

---

## 4. Host Key Verification

SSH Host Key 必须通过配置的：

```text
known_hosts_file
```

验证。

实现使用严格 `known_hosts` 匹配：

```text
host + port + server public key
```

以下情况全部 Fail Closed：

- `known_hosts` 文件不存在。
- Host Key 不匹配。
- Host Key 被替换。
- Host Key 校验过程失败。

不存在：

```text
accept_any_host_key
skip_host_key_check
insecure=true
```

之类的绕过路径。

---

## 5. Authentication

M2 支持：

```text
Password
Private Key
Encrypted Private Key + passphrase_secret_ref
```

### Password

```text
secret_ref
  ↓
SecretResolver
  ↓
authenticate_password
```

### Private Key

私钥文件在本地加载。

如果私钥有 Passphrase：

```text
passphrase_secret_ref
        ↓
SecretResolver
        ↓
load_secret_key
        ↓
authenticate_publickey
```

私钥解析放到 blocking task，不阻塞 Tokio Runtime Worker。

---

## 6. Read-only SFTP Transport

Transport 只通过 SSH 打开：

```text
session channel
      ↓
SFTP subsystem
```

没有打开远程 Shell。

当前生产 API 只暴露：

```text
stat
lstat
read_dir
read_range(offset, length)
close
```

`read_range` 有硬上限：

```text
MAX_READ_RANGE_BYTES = 4 MiB
```

同时验证 offset + length 不发生整数溢出。

### 6.1 明确不存在的能力

M2 不实现：

```text
write
create
truncate
rename
remove
mkdir
exec
shell
PTY
scp upload
```

因此 Log Query MCP 的 SSH Transport 不是通用 SSH MCP，也不是远程运维 Shell。

---

## 7. Timeout / Broken Session

连接阶段和操作阶段分别使用：

```text
connect_timeout_millis
operation_timeout_millis
```

SFTP 操作发生以下情况时：

```text
protocol / IO failure
operation timeout
network disconnect
```

当前 Reader 会进入：

```text
Broken
```

状态。

后续任何操作立即返回：

```text
SshTransportError::Broken
```

不会继续尝试复用已经失去可信状态的 Session。

Reader 被 Drop 后，对应全局连接 Permit 会释放。

---

## 8. Error Redaction

生产 Transport Error 使用稳定分类：

```text
InvalidConfiguration
UnknownConnection
ConnectionLimit
ConnectTimeout
ConnectFailed
HostKeyVerificationFailed
AuthenticationFailed
SecretUnavailable
KeyLoadFailed
OperationTimeout
SshProtocol
SftpProtocol
Broken
InvalidRemotePath
InvalidReadRange
```

这些错误不会把以下内容拼进返回字符串：

```text
password
private key content
secret value
remote absolute path
username credentials
```

M5 接 MCP Tool Error 时仍需要继续执行对外去敏映射。

---

## 9. M2 实机测试矩阵

SSH Transport Workflow 使用临时 OpenSSH Server + internal-sftp 进行真实协议测试。

已覆盖：

1. 正确 Password 登录。
2. 错误 Password 拒绝，并验证错误字符串不包含密码。
3. 正确 encrypted Private Key + Passphrase 登录。
4. 错误 Private Key 拒绝。
5. 正确 Host Key。
6. `known_hosts` 缺失 Fail Closed。
7. Host Key 改变 Fail Closed。
8. 无权限文件失败且不泄漏路径。
9. 不存在文件失败且不泄漏路径。
10. 大文件 Offset Range Read，只读取请求范围。
11. SFTP 操作超时后 Reader 标记 Broken。
12. 网络中途断开后 Reader 标记 Broken。
13. Connect Future 被取消后 Semaphore Permit 可靠释放。

其中成功路径同时覆盖：

```text
stat
lstat
read_dir
seek + read range
```

---

## 10. Dependency Lock

SSH/SFTP 依赖已加入生产 `Cargo.toml` 并固化进 `Cargo.lock`。

锁文件由经过生产编译和实机测试的 CI 结果生成，然后单独提交：

```text
e406ac76be29fb679c7538b816894607dadbbd23
chore: lock ssh transport dependencies
```

用于持久化 lockfile 的临时 Write Job 已在完成后删除。

当前 SSH Transport Workflow 恢复为：

```text
permissions:
  contents: read
```

并执行：

```text
cargo check --locked --all-targets --all-features
```

因此后续 CI 不允许隐式修改依赖版本。

---

## 11. M2 最终 Gate

M2 代码基线 `2e4110ba2c6ebe1cff5d2c9d1dab2a9aa57046ce` 已通过：

```text
Rust
  cargo fmt --all -- --check                         PASS
  cargo clippy --locked --all-targets --all-features -- -D warnings  PASS
  cargo test --locked --all-targets --all-features  PASS
  cargo build --release --locked --bins              PASS

Contracts
  v1 + v2 contract validation                        PASS

SSH Transport
  cargo fmt                                           PASS
  cargo check --locked --all-targets --all-features  PASS
  production SSH/SFTP live matrix                     PASS
  research POC compatibility                          PASS
```

验证运行：

```text
Rust:          31169380573
Contracts:     31169380540
SSH Transport: 31169380548
```

---

## 12. M2 Gate 结论

满足 M2 Gate：

```text
SSH/SFTP 可稳定读取受控文件范围
不存在远程命令执行能力
Host Key 验证不能绕过
故障 Session 不继续复用
Secret 不进入配置明文、Debug 或错误字符串
依赖由 Cargo.lock 锁定
```

M2 完成后，Remote SSH 仍然没有接入现有 MCP 查询路径。

这是刻意保持的阶段边界。

---

# 13. 下一阶段：M3 CacheStore

下一步不是直接让 Query Engine 读取远程 SSH 文件。

先实现本地 CacheStore：

```text
src/cache/
├── mod.rs
├── store.rs
├── manifest.rs
├── generation.rs
└── gc.rs
```

M3 的核心目标：

```text
可恢复
有界
原子写入
多 generation
活动 Snapshot 安全
```

关键边界：

- Cache 本地路径只从内部 ID 生成，不直接拼接远程绝对路径。
- Cache Directory `0700`。
- Cache File `0600`。
- Manifest 有版本并支持重启恢复。
- 写入采用 staging + atomic commit。
- truncate / replacement / continuity mismatch 创建新 Generation。
- Quota / retention / generations 数量都有硬限制。
- 活动 Snapshot / cursor / match_ref 引用的 Generation 不允许被 GC。

M3 完成后再进入 M4 SyncEngine，最后 M5 才把 Remote Source 接入现有 Query Engine。
