# ADR-0011：Remote Transport 使用 russh 与 russh-sftp

- 状态：Accepted for v2
- 日期：2026-08-07

## 决策

1. v2 Remote SSH Transport 使用 Tokio-native 的 `russh` 与 `russh-sftp` 作为首选实现基础。
2. 业务代码不直接暴露通用 SSH Handle、Channel 或 SFTP Session，而是封装为只读日志传输接口。
3. 内部接口只允许日志同步所需能力：`stat`、`lstat`、`read_dir`、read-only open 和有界 `read_range`。
4. 不在 Transport 抽象中提供 `exec`、shell、write、create、truncate、rename、remove 或上传能力。
5. Password、Private Key、encrypted private key + passphrase 与 `known_hosts` Host Key Verification 均作为 v2 正式能力。
6. connect/auth/SFTP operation 使用应用层 `tokio::time::timeout` 建立统一 deadline；库自身 inactivity/keepalive 作为补充保护。
7. 项目自身继续保持 `unsafe_code = "forbid"`；第三方 SSH/crypto 依赖内部实现的 unsafe/native 边界通过依赖审计管理，不把“整个依赖树零 unsafe”作为 v2 前提。
8. 生产实现添加依赖时锁定当时验证通过的兼容 patch 版本并提交 `Cargo.lock`，不使用 git HEAD dependency。

## 证据

M0 建立了隔离的 `research/ssh-sftp-poc` 和 GitHub Actions `SSH Research`，使用临时 OpenSSH Server 实际验证：

- POC 可以在 `unsafe_code = "forbid"` 下编译。
- Password Authentication 成功。
- 加密 Private Key + passphrase Authentication 成功。
- SFTP `stat/lstat/readdir/open/seek/read` 成功。
- `known_hosts` 缺失目标 host key 时连接 fail-closed。
- SSH 握手未完成时应用层 connect timeout 生效。

更深的 read 中断、server disconnect、operation cancellation 和连接池资源释放属于 M2 Transport 故障注入验收，不改变本 ADR 的库选型。

## 原因

当前项目基于 Tokio。`russh` 与 `russh-sftp` 可以直接使用异步 SSH/SFTP，并允许通过 SFTP subsystem 获取日志，无需服务器执行 Shell。相比同步/FFI 风格的 SSH binding，这种方式更容易把网络 deadline、取消和只读能力边界放进现有异步架构。

直接封装窄接口而不是采用通用 SSH 客户端 API，也能让“Log Query MCP 不具备 Remote Exec”成为代码结构约束，而不是仅靠调用约定。

## 后果

- M2 需要实现自己的 `SshConnectionManager` / `RemoteLogReader` 薄封装。
- 需要对第三方依赖持续执行 license/security audit。
- 首次 CI 编译 crypto/SSH 依赖成本高于当前 v1；后续可增加 Cargo cache 优化 CI。
- Remote Transport 的网络层可以保持跨平台，但 v2 MVP 整体运行平台继续沿用现有 Linux 基线。
