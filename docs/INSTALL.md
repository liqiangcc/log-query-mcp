# Log Query MCP 生产安装指南

本文说明如何从正式发布包安装、配置、启动、验证、升级、回滚和卸载 Log Query MCP。v2 同时支持 Local Source 与 Remote SSH/SFTP Source；Remote SSH 可使用 Direct TCP 或管理员配置的 ProxyCommand；AI-facing MCP 工具保持不变。

## 1. 前置条件

MCP 主机要求：

- Linux kernel `>= 5.6`。
- 首批发布目标为 `x86_64-unknown-linux-gnu` glibc 环境。
- systemd。
- `curl`，用于标准 MCP protocol health check。
- root 或 sudo 权限用于安装。
- 默认只需要本机 loopback 访问 MCP endpoint。
- Remote Direct 模式需要 MCP 运行环境可直接连接目标 SSH Server。
- Remote ProxyCommand 模式需要管理员安装并批准一个能把 stdin/stdout 映射到目标 TCP 字节流的本地/宿主机 helper。
- 足够的本地 cache 空间。

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
VERSION=0.2.0
TARGET=x86_64-unknown-linux-gnu
BASE_URL="https://github.com/liqiangcc/log-query-mcp/releases/download/v${VERSION}"

curl -fL -O "${BASE_URL}/log-query-mcp-v${VERSION}-${TARGET}.tar.gz"
curl -fL -O "${BASE_URL}/SHA256SUMS"
sha256sum -c SHA256SUMS
```

可进一步执行完整 package validator：

```bash
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
schemas/log-query-mcp-config-v2.schema.json
systemd/log-query-mcp.service
scripts/install.sh
scripts/uninstall.sh
scripts/healthcheck.sh
scripts/upgrade.sh
scripts/rollback.sh
docs/INSTALL.md
docs/OPERATIONS.md
docs/PRODUCTION_CHECKLIST.md
docs/CONFIG_SCHEMA_V2.md
docs/PROXY_COMMAND_TRANSPORT_V2.md
docs/M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md
docs/M7_PROXY_COMMAND_LIVE_GATE_V2.md
docs/M7_PROXY_AUTH_GATE_V2.md
docs/M7_PROXY_SYNC_GATE_V2.md
docs/M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md
docs/M7_PROXY_RESTART_GATE_V2.md
docs/M7_PROXY_GENERATION_GATE_V2.md
docs/M7_PROXY_PERFORMANCE_GATE_V2.md
docs/M6_PERFORMANCE_BASELINE_V2.md
docs/M6_FINAL_BASELINE_V2.md
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

安装器不会自动启动服务。先完成配置、Secret、known_hosts、Proxy helper（如使用）和权限检查，再启动。

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

该示例同时包含 Direct SSH 和 ProxyCommand 连接。生产环境只保留实际需要的 connection/source。

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

Remote connection 的 host、port、username、root、credential reference 和 ProxyCommand 都是管理员配置，不由 MCP 请求提供。

### 5.1 Password Secret

配置只保存：

```text
secret_ref=ORDER_LOG_PASSWORD
```

实际 Secret 通过 systemd environment / environment file / 上层 Secret 管理方式提供，不把明文密码写进 config、仓库或 journal。

### 5.2 Private key

Private key 文件只授予 `log-query-mcp` 服务用户必要读取权限。加密 key 的 passphrase 使用 `passphrase_secret_ref`，不写入普通配置，也不传给 ProxyCommand helper。

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

Host key 变化默认 fail-closed。不要配置“自动接受新 key”。无论 Direct 还是 ProxyCommand，known_hosts 身份都针对配置中的逻辑目标 `host:port`，不是 helper 进程或宿主机。

### 5.4 ProxyCommand / WSL

ProxyCommand 只在 Direct TCP 不适用而管理员控制的本地/宿主机网络路径可用时启用。配置形态：

```json
{
  "connection_id": "inventory-vpn-proxy-ssh",
  "type": "ssh",
  "host": "inventory-vpn.internal",
  "port": 22,
  "username": "log-reader",
  "auth": {
    "type": "password",
    "secret_ref": "LOG_QUERY_MCP_INVENTORY_PASSWORD"
  },
  "host_key": {
    "known_hosts_file": "/etc/log-query-mcp/known_hosts"
  },
  "proxy": {
    "type": "command",
    "program": "ncat.exe",
    "args": ["{host}", "{port}"]
  }
}
```

规则：

- `program` 与 `args` 只能由管理员静态配置。
- 不使用 Shell command string；程序直接 spawn。
- 只有完整 argv 元素 `{host}` / `{port}` 会被替换。
- 不允许 credential、username、remote path、Secret 或 MCP 请求进入 placeholder。
- helper stdout 必须只输出目标 TCP 字节流；诊断写 stderr。
- SSH password/private-key/passphrase 仍由 SSH 层处理。
- ProxyCommand 失败不会回退成 remote shell，也不会绕过 strict known_hosts。

WSL 典型验收：

```text
WSL log-query-mcp
  → Windows executable helper
  → Windows/VPN network stack
  → target SSH
  → SFTP read-only
  → local cache
```

安装前先以**与服务相同的运行身份**验证 helper：

- helper 可执行并可通过宿主机/VPN 到达目标；
- `program` 最好使用明确的管理员批准路径，避免依赖交互式 shell 的 PATH；
- WSL Windows executable interop 对 systemd 服务身份可用；
- 当前 systemd hardening（`NoNewPrivileges`、`ProtectSystem`、`ProtectHome`、`RestrictAddressFamilies` 等）没有被为 helper 整体放宽；若 helper 与 hardening 冲突，必须评估最小化调整，而不是关闭全部保护；
- 目标验收中应证明 Direct path 确实不可用而 ProxyCommand path 可用。

不要为了方便配置：

```text
program = sh / bash / powershell
args = dynamic command string
credential in argv
dynamic MCP-supplied command
```

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

超大日志建议优先使用 Tail bootstrap。M6 工程基线中 10 GiB logical file 使用 64 MiB Tail，只同步/cache 约 64 MiB，而不是完整 10 GiB。该数字是回归基线，不是生产 SLA。M7 ProxyCommand 沿用相同 transfer/cache 不变量，当前 paired performance harness 尚待真实 runner 执行。

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

如果使用 ProxyCommand，启动前同时确认服务用户可执行 helper；不要只在管理员的交互式 shell 中测试。

## 9. MCP 基础验证

发布包标准健康检查：

```bash
sudo scripts/healthcheck.sh
```

它要求 systemd 服务 active，并向 `/mcp` 发送 MCP `initialize`，验证 `jsonrpc=2.0`、`serverInfo` 和 `log-query-mcp` 服务身份。如果进程存活但协议返回错误，健康检查会失败。

人工协议验证：

```bash
curl -sS http://127.0.0.1:8000/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual-smoke","version":"0.2.0"}}}'
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

- `list_log_sources` 不返回 SSH host/username/secret_ref/cache path/ProxyCommand argv。
- 已知 Remote 日志可以搜索。
- `get_log_context` 正常从本地 snapshot 读取。
- Tail/FromNow 历史范围不足会显式返回 `CACHE_SCOPE_EXCEEDED`。
- 无 Remote Exec/写操作入口。
- ProxyCommand source 的失败不会泄露完整 argv、raw stderr 或 OS error。

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
7. 执行 `healthcheck.sh`，同时验证 systemd 和 MCP protocol；
8. restart 或 protocol health 失败时自动 rollback。

如使用非默认 endpoint，可通过 `LOG_QUERY_MCP_URL` 调整 health-check URL。生产环境不建议关闭 systemd 检查；`LOG_QUERY_MCP_HEALTHCHECK_SKIP_SYSTEMD=1` 只用于明确的容器/测试场景。

升级完成后还应执行 Local/Direct/Proxy smoke query 和 `get_log_context`。如果生产配置使用 ProxyCommand，升级验收必须以服务身份重新验证 helper 生命周期和目标连通性。

## 13. 回滚

升级日志会输出 backup path。显式回滚：

```bash
sudo scripts/rollback.sh /var/lib/log-query-mcp/backups/<backup-dir>
```

Rollback 恢复 upgrade 前的 binaries、BUILDINFO、config、systemd unit，然后 restart，并要求恢复后的服务通过相同的 MCP protocol health check。

若自动 rollback 也失败，停止继续升级，保留 backup，人工检查磁盘、权限、systemd、配置和 Secret。

## 14. Host key rotation / Secret rotation

Host key rotation：先独立核对新 fingerprint，再替换 known_hosts，随后重启/查询验证。

Secret rotation：更新 Secret 来源后重启服务，使用一个已知 Remote query 验证。不要在诊断输出中打印 Secret 值。ProxyCommand 不需要也不应该接收新 Secret。

## 15. Release Candidate 本地检查

当需要在 GitHub Actions 之外执行全部仓库本地 Gate：

```bash
bash scripts/rc_check.sh
```

该脚本执行 Contracts、ProxyCommand release contract、rustfmt、Clippy、全量测试、release build、protocol health-check 负向矩阵、upgrade/rollback 演练、release package 生成与 validator。package validator 要求 v2 示例同时包含 Direct/ProxyCommand，并要求 tarball 包含 v2 machine schema 与 M7 交付文档。

它不替代真实 Direct/Proxy SSH live Gate、M7 performance Gate、真实 WSL → Windows Host acceptance 或目标生产环境验收。

## 16. 卸载

保留配置：

```bash
sudo scripts/uninstall.sh
```

连同配置：

```bash
sudo scripts/uninstall.sh --purge-config
```

卸载脚本不会删除 `log-query-mcp` 用户/组，也不应自动删除需要保留取证的 backup/cache；生产清理应按审批和 retention 策略执行。
