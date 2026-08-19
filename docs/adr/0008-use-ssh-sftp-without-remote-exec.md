# ADR-0008：Remote Source 仅使用 SSH/SFTP，不提供远程命令执行

- 状态：Accepted for v2
- 日期：2026-08-07

## 决策

1. Remote Source 通过 SSH 建立安全连接，并通过 SFTP 执行受控的目录枚举、metadata 查询和文件范围读取。
2. Log Query MCP 不提供 `ssh_exec`、`run_shell`、远程 `grep`、远程 `find`、任意文件读取或其他通用 SSH 工具。
3. v2 MVP 支持 Password 和 Private Key 两类 SSH 认证；密码和私钥口令必须通过 Secret Resolver 或受控本地文件获取，不允许把明文密码作为普通配置字段。
4. SSH Host Key Verification 默认强制开启；配置必须提供受信任的 `known_hosts` 来源，不提供默认的 accept-all / insecure 模式。
5. 推荐远程服务器使用专用只读账号 `log-reader`，无 sudo、无日志写权限；生产环境推荐进一步使用 SFTP-only 和 chroot。
6. MCP 客户端永远不能读取 host、username、secret_ref、私钥路径、远程绝对路径等连接敏感信息。

## 原因

项目的核心安全价值是“AI 只能查询管理员预先授权的日志来源”。一旦提供任意 SSH 命令执行或客户端可提交远程路径，该边界会退化为通用远程运维权限，并引入 Shell 注入、权限扩大和横向访问风险。

SFTP 足以支持日志获取，同时可以避免依赖服务器上的 `grep`、`awk`、`tail` 等命令和 Shell 行为差异。

## 后果

- 大文件首次同步可能产生较大网络流量，必须通过 Bootstrap 策略和增量同步控制。
- Remote Source 的路径安全无法完全复制本机 `openat2()`；必须结合 MCP 配置校验、SFTP `lstat`、远程 Unix 权限和推荐 chroot 建立纵深防御。
- 部署、文件上传、服务重启等能力应由独立的 Deployment/SSH MCP 提供，不进入本项目职责范围。
