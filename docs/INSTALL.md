# Log Query MCP 生产安装指南

本文说明如何从正式发布包安装、配置、启动、验证、升级、回滚和卸载 Log Query MCP。

## 1. 前置条件

目标服务器要求：

- Linux kernel `>= 5.6`。
- `x86_64-unknown-linux-gnu` glibc 环境。
- systemd。
- root 或 sudo 权限。
- 能够读取目标日志目录。
- 默认只需要本机 loopback 网络访问。

首期发布只提供 `tar.gz + systemd`，不提供 `.deb`、RPM 或 OCI image。

## 2. 下载和校验发布包

```bash
VERSION=0.1.0
TARGET=x86_64-unknown-linux-gnu
BASE_URL="https://github.com/liqiangcc/log-query-mcp/releases/download/v${VERSION}"

curl -fL -O "${BASE_URL}/log-query-mcp-v${VERSION}-${TARGET}.tar.gz"
curl -fL -O "${BASE_URL}/SHA256SUMS"
sha256sum -c SHA256SUMS
tar -xzf "log-query-mcp-v${VERSION}-${TARGET}.tar.gz"
cd "log-query-mcp-v${VERSION}-${TARGET}"
```

包内应包含：

```text
bin/log-query-mcp
bin/log-query-mcp-stdio
examples/log-query-mcp.v1.json
systemd/log-query-mcp.service
scripts/install.sh
scripts/uninstall.sh
docs/INSTALL.md
docs/OPERATIONS.md
docs/PRODUCTION_CHECKLIST.md
BUILDINFO
SHA256SUMS
```

如需校验包内文件：

```bash
sha256sum -c SHA256SUMS
cat BUILDINFO
```

## 3. 安装文件

```bash
sudo scripts/install.sh
```

安装脚本会：

- 创建系统用户和组 `log-query-mcp`。
- 复制二进制到 `/opt/log-query-mcp/bin`。
- 如果包内存在 `BUILDINFO`，复制到 `/opt/log-query-mcp/BUILDINFO`。
- 如果 `/etc/log-query-mcp/config.json` 不存在，写入示例配置。
- 安装 `/etc/systemd/system/log-query-mcp.service`。
- 执行 `systemctl daemon-reload`。

安装脚本不会自动启动服务。先检查配置和日志权限，再启动。

## 4. 配置日志来源

编辑配置：

```bash
sudoedit /etc/log-query-mcp/config.json
```

关键规则：

- `version` 必须为 `1`。
- `source_id` 是客户端可见的稳定来源 ID。
- `root` 必须是绝对路径，且不能是符号链接。
- `files` 和 `directories` 只能授权 `root` 下的相对路径。
- 不要把敏感或无关目录加入白名单。
- 根据日志大小调整 `limits`，尤其是 `max_scan_bytes_per_page`、`query_timeout_millis`、`max_response_bytes` 和 `max_concurrent_scans`。

示例配置见 `/etc/log-query-mcp/config.json` 或 [examples/log-query-mcp.v1.json](../examples/log-query-mcp.v1.json)。

配置更新后保持权限：

```bash
sudo chown root:log-query-mcp /etc/log-query-mcp/config.json
sudo chmod 0640 /etc/log-query-mcp/config.json
```

## 5. 授权日志读取权限

服务以 `log-query-mcp` 用户运行。该用户必须能进入日志目录并读取白名单文件。

用 ACL 授权示例：

```bash
sudo setfacl -m u:log-query-mcp:rx /var/log/payment-service
sudo setfacl -m u:log-query-mcp:r /var/log/payment-service/application.log
sudo setfacl -m u:log-query-mcp:r /var/log/payment-service/application.log.1
```

用 Unix 组授权示例：

```bash
sudo usermod -aG adm log-query-mcp
sudo systemctl restart log-query-mcp.service
```

具体权限方案应由运维根据日志目录所有权和轮转策略确定。不要为了服务读取日志而把日志目录改成全局可读。

## 6. 启动服务

```bash
sudo systemctl enable --now log-query-mcp.service
sudo systemctl status --no-pager log-query-mcp.service
```

默认环境变量由 systemd unit 设置：

```text
LOG_QUERY_MCP_CONFIG=/etc/log-query-mcp/config.json
LOG_QUERY_MCP_BIND=127.0.0.1:8000
```

确认只监听 loopback：

```bash
ss -ltn | grep '127.0.0.1:8000'
```

查看日志：

```bash
journalctl -u log-query-mcp.service -n 100 --no-pager
```

## 7. 基础验证

执行 MCP 初始化请求：

```bash
curl -sS http://127.0.0.1:8000/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{
    "jsonrpc":"2.0",
    "id":1,
    "method":"initialize",
    "params":{
      "protocolVersion":"2025-06-18",
      "capabilities":{},
      "clientInfo":{"name":"manual-smoke","version":"0.1.0"}
    }
  }'
```

期望返回包含 `serverInfo.name` 为 `log-query-mcp` 的 JSON-RPC 响应。

使用 MCP Inspector 验证：

```bash
npx -y @modelcontextprotocol/inspector
```

打开 Inspector UI 后选择 Streamable HTTP，连接：

```text
http://127.0.0.1:8000/mcp
```

在 Tools 页确认存在：

```text
list_log_sources
search_logs
get_log_context
```

参考 MCP Inspector 官方文档：

- <https://modelcontextprotocol.io/docs/tools/inspector>
- <https://github.com/modelcontextprotocol/inspector>

## 8. AI 客户端配置

客户端配置示例：

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

如果 AI 客户端运行在其他主机，不要直接把服务暴露到公网。应通过内网 ACL、反向代理或上层网关提供认证、TLS 和访问控制，并显式配置非 loopback bind。

## 9. 调试 stdio 入口

stdio binary 用于本机调试，不作为生产 systemd 服务入口：

```bash
LOG_QUERY_MCP_CONFIG=/etc/log-query-mcp/config.json \
  /opt/log-query-mcp/bin/log-query-mcp-stdio
```

诊断日志写 stderr，stdout 仅用于 MCP stdio 协议。不要把普通日志写到 stdout。

## 10. 升级

1. 下载并校验新版本发布包。
2. 备份当前二进制和配置：

   ```bash
   sudo mkdir -p /opt/log-query-mcp/backups
   sudo cp -a /opt/log-query-mcp/bin "/opt/log-query-mcp/backups/bin-$(date +%Y%m%d%H%M%S)"
   sudo cp -a /etc/log-query-mcp/config.json "/etc/log-query-mcp/config.json.$(date +%Y%m%d%H%M%S).bak"
   ```

3. 停止服务：

   ```bash
   sudo systemctl stop log-query-mcp.service
   ```

4. 在新包目录执行：

   ```bash
   sudo scripts/install.sh
   ```

5. 检查配置没有被覆盖，启动并验证：

   ```bash
   sudo systemctl start log-query-mcp.service
   sudo systemctl status --no-pager log-query-mcp.service
   journalctl -u log-query-mcp.service -n 100 --no-pager
   ```

## 11. 回滚

使用上一版本发布包回滚：

```bash
sudo systemctl stop log-query-mcp.service
cd log-query-mcp-vOLD-x86_64-unknown-linux-gnu
sudo scripts/install.sh
sudo systemctl start log-query-mcp.service
```

如果只需要恢复备份二进制：

```bash
sudo systemctl stop log-query-mcp.service
sudo cp -a /opt/log-query-mcp/backups/bin-YYYYMMDDHHMMSS/* /opt/log-query-mcp/bin/
sudo systemctl start log-query-mcp.service
```

回滚后必须重新执行基础验证和至少一次真实日志查询。

## 12. 卸载

保留配置卸载：

```bash
sudo scripts/uninstall.sh
```

连同配置一起删除：

```bash
sudo scripts/uninstall.sh --purge-config
```

卸载脚本不会删除 `log-query-mcp` 用户和组。如需删除，确认没有其他流程依赖后手工处理。
