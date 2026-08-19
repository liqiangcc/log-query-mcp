# Log Query MCP v2：Rust SSH/SFTP 技术预研

> 状态：M0 技术预研结论  
> 日期：2026-08-07  
> 关联方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> 关联 TODO：[`REMOTE_SSH_CACHE_TODO_V2.md`](./REMOTE_SSH_CACHE_TODO_V2.md)

## 1. 结论

v2 Remote Source 推荐直接使用：

```text
russh 0.62.x
+
russh-sftp 2.x
+
Tokio
```

不采用 `ssh2`/libssh2 作为首选，也不在业务层采用允许任意命令执行的高层 SSH API。

核心原因：

- `russh` 原生基于 Tokio async，和当前项目运行时一致。
- `russh` 原生支持 Password、Public Key、encrypted private key、known_hosts、keepalive。
- `russh-sftp` 可以直接基于 SSH subsystem channel 工作，不需要远程 Shell。
- SFTP 支持 stat/lstat/readdir/open/read，并且底层 Raw API 支持显式 `offset + len` 读取，非常适合增量同步。
- 不依赖服务器安装任何 Agent/MCP。
- 可以在项目内部只封装只读 SFTP 能力，避免 `exec`/shell 成为可达业务能力。

建议 M2 实现时把依赖锁定到当时验证通过的 patch 版本并提交 `Cargo.lock`，而不是在设计文档中永久冻结 patch 号。

---

## 2. 候选方案比较

| 方案 | Tokio async | SFTP | known_hosts | Password/Key | 外部 native 依赖 | 结论 |
|---|---|---|---|---|---|---|
| `russh` + `russh-sftp` | 原生 | 原生生态 | 支持 | 支持 | crypto backend 存在 native/unsafe 边界 | 推荐 |
| `ssh2` | 非 Tokio 原生 | 支持 | 需要自行组合 | 支持 | libssh2/OpenSSL | 不首选 |
| `async-ssh2-tokio` | 原生 | 基于 russh-sftp | 支持 | 支持 | 继承 russh | 可参考，不作为核心封装 |
| 调用系统 `ssh`/`sftp` | 进程级 | 支持 | OpenSSH | 支持 | 依赖本机二进制 | 不采用 |

### 2.1 为什么不优先使用 `ssh2`

`ssh2` 是 libssh2 的 Rust binding。它成熟且文档完整，但当前项目已经是 Tokio async 架构，使用它意味着引入同步/FFI 边界或额外阻塞调度，同时还引入 libssh2/OpenSSL 构建复杂度。

Remote Log Sync 的核心需求不是完整 SSH 功能，而是受控的异步 SFTP range-read，因此直接使用 Tokio-native `russh` 更匹配。

### 2.2 为什么不直接使用高层 SSH Client

高层封装可以减少代码，但通常同时暴露：

```text
exec
shell
port forwarding
SFTP
```

Log Query MCP 的安全模型要求代码结构本身体现“没有 Remote Exec”。

因此 M2 应封装自己的：

```text
SshReadTransport
```

只暴露：

```text
stat
lstat
readdir
open_readonly
read_range
close
```

---

## 3. Password Authentication

`russh::client::Handle` 提供异步：

```text
authenticate_password(username, password)
```

满足 v2 Password Authentication 要求。

密码来源必须经过 `SecretResolver`，Transport 不接受配置对象中的明文 password 字段。

建议调用边界：

```text
Config secret_ref
      ↓
SecretResolver
      ↓
Secret<String>
      ↓
authenticate_password
```

Secret 类型不得实现会泄漏内容的 Debug/Display。

---

## 4. Private Key Authentication

`russh` 提供：

```text
load_secret_key(path, password)
```

可以读取并解密私钥。

认证使用：

```text
PrivateKeyWithHashAlg
+
authenticate_publickey
```

RSA key 可以通过：

```text
best_supported_rsa_hash()
```

选择服务器支持的签名 hash。

因此 v2 的：

```text
private_key
passphrase_secret_ref
```

技术路径成立。

---

## 5. Encrypted Private Key

`load_secret_key(path, Some(passphrase))` 和 `decode_secret_key(..., Some(passphrase))` 支持加密私钥解密。

M2 必须：

- passphrase 从 `SecretResolver` 获取。
- 不把 passphrase 保存在连接池 metadata。
- 不记录私钥内容。
- 不把底层解密错误原样暴露给 MCP Client。

MCP 统一映射为：

```text
REMOTE_AUTH_FAILED
```

详细原因只能进入去敏后的本地诊断日志。

---

## 6. Host Key Verification

`russh` Client Handler 提供：

```text
check_server_key(server_public_key)
```

`russh::keys::known_hosts` 提供：

```text
check_known_hosts(host, port, key)
check_known_hosts_path(host, port, key, path)
```

v2 使用管理员指定的：

```text
known_hosts_file
```

因此 Handler 应执行：

```text
check_known_hosts_path(...)
```

规则必须是 fail-closed：

- 匹配：允许连接。
- 主机不存在：拒绝。
- known_hosts 文件不存在：拒绝。
- key changed：拒绝。
- known_hosts 解析异常：拒绝。

不得调用自动学习 host key 的 API。

也不得提供：

```text
accept_any_host_key = true
```

之类的生产配置。

---

## 7. SFTP Subsystem

`russh` 官方 SFTP 示例使用：

```text
channel_open_session
      ↓
request_subsystem("sftp")
      ↓
channel.into_stream()
      ↓
russh_sftp::client::SftpSession
```

这里没有执行远程 Shell 命令。

这与 ADR-0008 的边界一致。

---

## 8. SFTP 能力映射

v2 所需能力均有对应技术路径。

| v2 能力 | russh-sftp 路径 |
|---|---|
| stat | `SftpSession::metadata` / Raw `stat` |
| lstat | `SftpSession::symlink_metadata` / Raw `lstat` |
| readdir | `SftpSession::read_dir` / Raw `readdir` |
| open read-only | `SftpSession::open` |
| seek/read | AsyncSeek/AsyncRead |
| exact range read | `RawSftpSession::read(handle, offset, len)` |

对于 SyncEngine，推荐优先设计成显式 range read：

```text
read_range(path, offset, length)
```

内部可使用 Raw SFTP 的：

```text
read(handle, offset, len)
```

原因：

- offset 是显式输入，易于审计。
- 更容易做最大单块读取限制。
- 更容易处理中断重试。
- 不依赖共享 cursor 状态。
- 与增量同步模型天然一致。

---

## 9. 禁止写能力

虽然 `russh-sftp` 库本身也提供 write/create/remove/rename 等 API，但 Log Query MCP 的 Transport wrapper 不得暴露它们。

推荐 trait 形态：

```rust
trait RemoteLogReader {
    async fn stat(...);
    async fn lstat(...);
    async fn read_dir(...);
    async fn read_range(...);
}
```

不要设计成通用：

```text
RemoteFileSystem
```

否则未来容易把写操作顺手暴露进来。

安全边界应该由类型/接口限制，而不仅仅靠“调用方约定不用”。

---

## 10. Timeout 模型

`russh::client::Config` 提供：

```text
inactivity_timeout
keepalive_interval
keepalive_max
```

但 v2 配置已有：

```text
connect_timeout_millis
operation_timeout_millis
```

建议不要完全依赖库内部 timeout。

统一在应用层使用：

```text
tokio::time::timeout
```

包裹：

```text
connect
authentication
open SFTP subsystem
stat/lstat/readdir
read_range
```

这样能够保证所有 Transport operation 都受到同一 deadline 模型控制。

SFTP 自身的 response timeout 可以作为第二层保护，而不是唯一超时机制。

---

## 11. Cancellation

SSH/SFTP 操作都是 async future，应用层 timeout/cancellation 可以通过 future drop 传播。

但 M2 必须增加实际测试，确认：

- timeout 后没有悬挂的 query task。
- SSH channel 能释放。
- SFTP session 能释放。
- connection pool 不会把已损坏连接重新放回池。
- shutdown 时没有阻塞线程无法退出。

建议 ConnectionManager 对连接维护显式状态：

```text
Healthy
Closing
Broken
```

任何 transport/protocol error 都应将当前连接标记为 `Broken`，而不是盲目复用。

---

## 12. Blocking 边界

SSH 网络路径本身使用 Tokio async。

但以下操作可能包含同步文件 I/O：

```text
读取 known_hosts
读取 private key
```

它们是小文件操作，但为了避免阻塞 async worker，M2 推荐：

```text
tokio::task::spawn_blocking
```

处理本地私钥/known_hosts 文件加载，或者在建立连接前同步加载并缓存不可变解析结果。

禁止把大日志扫描放进 SSH async task；日志扫描仍走现有受限 scanner/executor。

---

## 13. Connection Pool

MVP 不需要复杂数据库式连接池。

推荐：

```text
ConnectionManager
  connection_id
      ↓
  max 1~N healthy SSH sessions
```

约束：

- 全局 semaphore。
- 每 connection_id 并发上限。
- idle timeout。
- broken session 立即淘汰。
- authentication failure 不重试风暴。
- host key failure 永不自动重试并接受新 key。

SyncEngine 应尽量在一次刷新中复用同一 SFTP Session 完成：

```text
readdir/stat
+
read_range
```

---

## 14. 依赖与安全边界

### russh

- License：Apache-2.0。
- Tokio-native。
- 当前 0.62 系列持续维护。
- 默认 crypto backend 包含 native/底层实现。
- russh 自身文档明确说明 `cryptovec` 内部使用 unsafe。

### russh-sftp

- License：Apache-2.0。
- 2.x 系列持续维护。
- Tokio async SFTP client/server implementation。

### unsafe_code = forbid 的解释

当前项目：

```toml
[lints.rust]
unsafe_code = "forbid"
```

该规则继续保持，意味着：

> Log Query MCP 自己的 crate 不新增 unsafe code。

第三方 crypto/SSH dependency 内部可能存在经其维护者封装的 unsafe/native 实现，这与 Rust 常见 crypto/network dependency 模型一致。

如果要求“整个依赖树绝对零 unsafe”，当前成熟 SSH/crypto 生态基本无法现实满足，不应把这个目标和本项目 `unsafe_code = forbid` 混为一谈。

M2 合并前建议增加：

```text
cargo tree
cargo deny check
cargo audit
```

检查依赖 License 和公开漏洞。

---

## 15. 平台兼容性

Remote Transport 本身不应依赖 Linux `openat2()`。

因此：

```text
SSH/SFTP Transport
```

应保持平台无关。

但当前 log-query-mcp v1 整体产品基线仍然是 Linux，并且 Local Source 使用 Linux `openat2()`。

所以 v2 MVP 建议暂时定义：

```text
Local MCP runtime: Linux
Remote server: any OpenSSH/SFTP-compatible Linux host
```

原生 Windows/macOS 本地运行属于后续平台化工作，不应在 M0/M1 顺手改变 v1 文件安全模型。

在 Windows 开发机上可以先通过 WSL/容器运行本地 MCP。

---

## 16. 版本策略

技术预研时验证的是：

```text
russh 0.62 series
russh-sftp 2.x series
```

M2 真正添加生产依赖时：

1. 使用当日最新兼容 patch。
2. `cargo update` 后提交 Cargo.lock。
3. CI 固定 `--locked`。
4. 不使用 git HEAD dependency。
5. 依赖升级走单独 PR/变更。

避免设计文档里的 patch 号变成长期错误的“版本真理”。

---

## 17. 推荐 M2 内部 API

```text
SshConnectionManager
    │
    └── open_reader(connection_id)
              │
              ▼
       RemoteLogReader
              │
              ├── stat(relative_path)
              ├── lstat(relative_path)
              ├── read_dir(relative_dir)
              └── read_range(relative_path, offset, len)
```

RemoteLogReader 不返回 SSH Handle，不返回 SftpSession，不暴露 Channel。

这样：

```text
SyncEngine
```

永远没有机会调用：

```text
exec
shell
write
remove
rename
```

---

## 18. M0 验证结论

### 已确认技术可行

- Password Authentication。
- Private Key Authentication。
- encrypted private key + passphrase。
- known_hosts Host Key Verification。
- SFTP stat/lstat/readdir/open。
- SFTP seek/read。
- SFTP exact offset range read。
- Tokio async 集成。
- connect/operation timeout 可以统一由应用层 timeout 控制。
- 不需要 Remote Exec。
- 本项目自身可继续保持 `unsafe_code = forbid`。

### M2 必须补实际故障注入测试

API/实现路径成立不等于所有故障语义已经经过实机验证。

M2 集成测试必须覆盖：

- SSH server 在 read 中途断开。
- authentication timeout。
- SFTP operation timeout。
- timeout future drop 后 session 是否可继续使用；不能继续使用时必须淘汰。
- host key changed。
- known_hosts missing。
- 日志文件在 range read 中 truncate/replace。
- MCP shutdown 时连接池释放。

这些属于 ConnectionManager/SFTP Transport 实现验收，不应阻塞 M1 的纯 Backend 抽象。

---

## 19. 参考资料

- russh docs: https://docs.rs/russh/
- russh client Handle: https://docs.rs/russh/latest/russh/client/struct.Handle.html
- russh known_hosts: https://docs.rs/russh/latest/russh/keys/known_hosts/
- russh official SFTP example: https://docs.rs/crate/russh/latest/source/examples/sftp_client.rs
- russh-sftp docs: https://docs.rs/russh-sftp/
- russh-sftp SftpSession: https://docs.rs/russh-sftp/latest/russh_sftp/client/struct.SftpSession.html
- russh-sftp RawSftpSession: https://docs.rs/russh-sftp/latest/russh_sftp/client/rawsession/struct.RawSftpSession.html
- ssh2 docs: https://docs.rs/ssh2/
- async-ssh2-tokio docs: https://docs.rs/async-ssh2-tokio/
