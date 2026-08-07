# Log Query MCP 生产安装指南

本文说明如何从正式发布包安装、配置、启动、验证、升级、回滚和卸载 Log Query MCP。v2 同时支持 Local Source 与 Remote SSH/SFTP Source；AI-facing MCP 工具保持不变。

## 1. 前置条件

MCP 主机要求：

- Linux kernel `>= 5.6`。
- 首批发布目标为 `x86_64-unknown-linux-gnu` glibc 环境。
- systemd。
- root 或 sudo 权限用于安装。
- 默认只需要本机 loopback 访问 MCP endpoint。
- Remote 模式需要到目标 SSH Server 的网络连通性和足够的本地 cache 空间。

首批发布只提供 `tar.gz + systemd`，不提供 `.deb`、RPM 或 OCI image。

Remote Server **不需要安装 MCP/Agent**。建议创建专用账号：

```text
log-reader
- no sudo
- read-only log permissions
- preferably SFTP-only / chroot when operationally practical
```

## 2. 下载和校验发布包

```bash
VERSION=0.1.0
TARGET=x86_64-unknown-linux-gnu
BASE_URL="https://github.com/liqiangcc/log-query-mcp/releases/download/v${VERSION}"

curl -fL -O "${BASE_URL}/log-query-mcp-v${VERSION}-${TARGET}.tar.gz"
curl -fL -O "${BASE_URL}/SHA256SUMS"
sha256sum -c SHA256SUMS
```

可进一步执行完整 package validator：

```bash
# validate_release_package.sh 可从同版本源码仓库或已解包 release scripts 中取得。
bash scripts/validate_release_package.sh \
  "log-query-mcp-v${VERSION}-${TARGET}.tar.gz" \
  SHA256SUMS
```

解包：

```bash
tar -xzf "log-query-mcp-v${VERSION}-${TARGET}.tar.gz"
cd "log-query-mcp-v${VERSION}-${TARGET}"
sha256sum -c SHA256SUMS
cat BUILDINFO
```

包内至少包含：

```text
bin/log-query-mcp
bin/log-query-mcp-stdio
examples/log-query-mcp.v1.json
examples/log-query-mcp.v2.remote.json
systemd/log-query-mcp.service
scripts/install.sh
scripts/uninstall.sh
scripts/upgrade.sh
scripts/rollback.sh
docs/INSTALL.md
docs/OPERATIONS.md
docs/PRODUCTION_CHECKLIST.md
docs/M6_PERFORMANCE_BASELINE_V2.md
docs/RELEASE_READINESS_V2.md
BUILDINFO
SHA256SUMS
```

## 3. 首次安装

```bash
sudo scripts/install.sh
```

安装脚本会：

- 创建系统用户和组 `log-query-mcp`。
- 复制二进制到 `/opt/log-query-mcp/bin`。
- 复制 `BUILDINFO` 到 `/opt/log-query-mcp/BUILDINFO`。
- 创建 `/var/lib/log-query-mcp/cache`。
- 如果 `/etc/log-query-mcp/config.json` 不存在，写入 v1 Local 示例配置。
- 安装 `/etc/systemd/system/log-query-mcp.service`。
- 执行 `systemctl daemon-reload`。

安装器不会自动启动服务。先完成配置、Secret、known_hosts 和权限检查，再启动。

## 4. Local Source 配置

v1 配置继续支持，适合 MCP 和日志在同一 Linux 主机：

```text
AI → MCP → LocalBackend → openat2() → log files
```

示例：

```bash
sudo cp examples/log-query-mcp.v1.json /etc/log-query-mcp/config.json
sudo chown root:log-query-mcp /etc/log-query-mcp/config.json
sudo chmod 0640 /etc/log-query-mcp/config.json
```

规则：

- `root` 必须是批准的绝对目录。
- 不配置 `/`、整个 `/var` 或与业务无关的宽目录。
- `files` / `directories` 只授权 root 下必要日志。
- Local 模式继续依赖 `openat2()`、symlink/magiclink/no-xdev 等安全边界。

## 5. Remote SSH Source 配置

Remote 模式使用 v2：

```bash
sudo cp examples/log-query-mcp.v2.remote.json /etc/log-query-mcp/config.json
sudo chown root:log-query-mcp /etc/log-query-mcp/config.json
sudo chmod 0640 /etc/log-query-mcp/config.json
```

架构：

```text
AI
 ↓
local log-query-mcp
 ↓
admin-configured SSH/SFTP read-only
 ↓
remote logs
 ↓
incremental local generation cache
 ↓
existing scanner/query engine
```

Remote connection 的 host、port、username、root、credential reference 都是管理员配置，不由 MCP 请求提供。

### 5.1 Password Secret

配置只保存：

```text
secret_ref=ORDER_LOG_PASSWORD
```

实际 Secret 通过 systemd environment / environment file / 上层 Secret 管理方式提供，不把明文密码写进 config、仓库或 journal。

### 5.2 Private key

Private key 文件只授予 `log-query-mcp` 服务用户必要读取权限。加密 key 的 passphrase 使用 `passphrase_secret_ref`，不写入普通配置。

### 5.3 known_hosts

先从网络获取候选 key：

```bash
ssh-keyscan -t ed25519 -p 22 server.example.internal > /tmp/server.known_hosts
ssh-keygen -lf /tmp/server.known_hosts
```

必须通过独立可信渠道核对 fingerprint，再安装：

```bash
sudo install -m 0644 /tmp/server.known_hosts /etc/log-query-mcp/known_hosts
```

Host key 变化默认 fail-closed。不要配置“自动接受新 key”。

## 6. Remote Server 账号权限

以 `log-reader` 为例，只授予目标日志目录的 read/traverse 权限，不授予 sudo、写、删除或部署权限。

条件允许时可使用 OpenSSH `internal-sftp` / chroot 进一步限制账号。具体 chroot 布局依系统目录结构制定；不要为了 chroot 破坏日志生产进程的权限模型。

Log Query MCP 不需要也不应依赖：

```text
ssh_exec
shell
remote grep
sudo
upload/write/delete
remote deploy/restart
```

## 7. Cache 容量

默认 cache root：

```text
/var/lib/log-query-mcp/cache
```

规划容量时考虑：bootstrap 范围、日志增长率、retention、多 generation、active cursor/match_ref pin 和安全余量。

超大日志建议优先使用 Tail bootstrap。M6 工程基线中 10 GiB logical file 使用 64 MiB Tail，只同步/cache 约 64 MiB，而不是完整 10 GiB。该数字是回归基线，不是生产 SLA。

## 8. 启动服务

```bash
sudo systemctl enable --now log-query-mcp.service
sudo systemctl status --no-pager log-query-mcp.service
ss -ltn | grep '127.0.0.1:8000'
journalctl -u log-query-mcp.service -n 100 --no-pager
```

默认：

```text
LOG_QUERY_MCP_CONFIG=/etc/log-query-mcp/config.json
LOG_QUERY_MCP_BIND=127.0.0.1:8000
```

如果 Remote Secret 通过 systemd EnvironmentFile 提供，确保文件仅 root/服务组按需可读，并执行：

```bash
sudo systemctl daemon-reload
sudo systemctl restart log-query-mcp.service
```

## 9. MCP 基础验证

```bash
curl -sS http://127.0.0.1:8000/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual-smoke","version":"0.1.0"}}}'
```

MCP Inspector 连接：

```text
http://127.0.0.1:8000/mcp
```

确认只有三个业务工具：

```text
list_log_sources
search_logs
get_log_context
```

Remote 验收还应确认：

- `list_log_sources` 不返回 SSH host/username/secret_ref/cache path。
- 已知 Remote 日志可以搜索。
- `get_log_context` 正常从本地 snapshot 读取。
- Tail/FromNow 历史范围不足会显式返回 `CACHE_SCOPE_EXCEEDED`。
- 无 Remote Exec/写操作入口。

## 10. AI 客户端配置

```json
{
  "mcpServers": {
    "log-query-mcp": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:8000/mcp"
    }
  }
}
```

不要直接暴露公网。非 loopback 使用必须由受控网关/内网 ACL 提供认证、TLS 和访问控制。

## 11. 调试 stdio 入口

```bash
LOG_QUERY_MCP_CONFIG=/etc/log-query-mcp/config.json \
  /opt/log-query-mcp/bin/log-query-mcp-stdio
```

stdout 只用于 MCP stdio，诊断写 stderr。

## 12. 安全升级

下载并验证新 release 外层 SHA256 后：

```bash
sudo scripts/upgrade.sh /path/to/log-query-mcp-vNEW-x86_64-unknown-linux-gnu.tar.gz
```

升级器会：

1. 在任何系统修改前校验包内 SHA256；
2. 在 `/var/lib/log-query-mcp/backups` 创建 rollback backup；
3. 备份当前 binaries、BUILDINFO、config、systemd unit；
4. 正常升级保持现有 production config，不用示例覆盖；
5. 使用同目录 temporary file + rename 原子替换 binaries/BUILDINFO/unit；
6. daemon-reload + restart；
7. health check 失败时自动 rollback。

升级完成后执行 MCP initialize、Local/Remote smoke query 和 `get_log_context`。

## 13. 回滚

升级日志会输出 backup path。显式回滚：

```bash
sudo scripts/rollback.sh /var/lib/log-query-mcp/backups/<backup-dir>
```

Rollback 恢复 upgrade 前的 binaries、BUILDINFO、config、systemd unit，然后 restart/health check。

若自动 rollback 也失败，停止继续升级，保留 backup，人工检查磁盘、权限、systemd、配置和 Secret。

## 14. Host key rotation / Secret rotation

Host key rotation：先独立核对新 fingerprint，再替换 known_hosts，随后重启/查询验证。

Secret rotation：更新 Secret 来源后重启服务，使用一个已知 Remote query 验证。不要在诊断输出中打印 Secret 值。

## 15. 卸载

保留配置：

```bash
sudo scripts/uninstall.sh
```

连同配置：

```bash
sudo scripts/uninstall.sh --purge-config
```

卸载脚本不会删除 `log-query-mcp` 用户/组，也不应自动删除需要保留取证的 backup/cache；生产清理应按审批和 retention 策略执行。
