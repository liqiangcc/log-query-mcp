# Log Query MCP 生产运维指南

本文面向负责运行 Log Query MCP 的运维和开发人员，覆盖日常操作、监控、配置变更、升级回滚和故障排查。

## 1. 服务模型

默认 systemd unit：

```text
/etc/systemd/system/log-query-mcp.service
```

默认运行身份：

```text
User=log-query-mcp
Group=log-query-mcp
```

默认环境变量：

```text
LOG_QUERY_MCP_CONFIG=/etc/log-query-mcp/config.json
LOG_QUERY_MCP_BIND=127.0.0.1:8000
```

HTTP endpoint 固定为：

```text
/mcp
```

v1 不提供独立 health endpoint。健康检查使用 systemd 状态、端口监听和 MCP 初始化请求。

## 2. 常用命令

```bash
sudo systemctl status --no-pager log-query-mcp.service
sudo systemctl restart log-query-mcp.service
sudo systemctl stop log-query-mcp.service
sudo systemctl start log-query-mcp.service
sudo systemctl disable --now log-query-mcp.service
```

查看日志：

```bash
journalctl -u log-query-mcp.service -f
journalctl -u log-query-mcp.service -n 200 --no-pager
```

确认监听：

```bash
ss -ltnp | grep ':8000'
```

确认二进制版本信息：

```bash
cat /opt/log-query-mcp/BUILDINFO
sha256sum /opt/log-query-mcp/bin/log-query-mcp /opt/log-query-mcp/bin/log-query-mcp-stdio
```

当前二进制没有单独的 `--version` 命令，发布版本以包名、GitHub Release tag 和 `BUILDINFO` 为准。

## 3. 配置变更

编辑配置：

```bash
sudoedit /etc/log-query-mcp/config.json
```

变更后重启服务：

```bash
sudo systemctl restart log-query-mcp.service
sudo systemctl status --no-pager log-query-mcp.service
```

建议每次配置变更记录：

- 变更人。
- 变更时间。
- 新增或删除的 `source_id`。
- 对应日志目录权限调整。
- 验证命令和结果。

配置文件应纳入服务器备份或配置管理。不要把敏感日志路径、内部服务名或访问策略提交到公开仓库。

## 4. 权限和安全

Log Query MCP 是只读日志查询服务，但返回内容可能包含敏感业务数据。生产使用必须遵守：

- 默认只监听 `127.0.0.1:8000`。
- v1 不内置认证和 TLS。
- 非 loopback 暴露必须经过内网 ACL、反向代理或上层网关。
- 只给 `log-query-mcp` 用户读取已批准日志的最小权限。
- 不要把日志目录改成全局可读。
- 不要把配置 root 指向过宽目录，例如 `/`、`/var` 或整个应用根目录。
- 定期复核 `sources` 白名单和 `limits`。

systemd unit 启用了基础加固：

- `NoNewPrivileges=true`
- `PrivateTmp=true`
- `ProtectSystem=strict`
- `ProtectHome=true`
- `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`

如果日志位于 home 目录或特殊挂载点，可能需要调整日志存放位置或 systemd 加固策略。优先移动日志到标准服务日志目录，而不是放宽服务权限。

## 5. 运行健康检查

系统状态：

```bash
sudo systemctl is-active log-query-mcp.service
```

端口状态：

```bash
ss -ltn | grep '127.0.0.1:8000'
```

协议初始化：

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
      "clientInfo":{"name":"ops-smoke","version":"0.1.0"}
    }
  }'
```

功能验证建议使用 MCP Inspector 或实际 AI 客户端执行：

1. `list_log_sources`，确认只返回已批准来源。
2. `search_logs`，使用一条已知 trace ID 或 request ID 查询。
3. `get_log_context`，使用上一步返回的 `match_ref` 获取有限上下文。

## 6. 监控建议

最低监控项：

- systemd 服务状态。
- 端口监听状态。
- journal 中的启动失败、配置读取失败、来源不可用、权限拒绝。
- 目标日志目录权限或挂载变化。
- 查询超时和资源限制错误的频率。

当前服务未暴露 Prometheus metrics。需要指标时，先通过 journal、systemd 和外部探测补齐，不要在 v1 内临时加入未设计的远程管理接口。

## 7. 日志轮转和文件替换

服务启动时会建立来源注册表并记录文件快照。日志文件被替换、删除或权限变化后，查询可能返回稳定错误，例如 `FILE_CHANGED` 或 `SOURCE_UNAVAILABLE`。

处理步骤：

1. 确认日志轮转是否符合配置中的 `files` 或 `directories` 规则。
2. 确认新文件权限允许 `log-query-mcp` 读取。
3. 重启服务刷新来源快照：

   ```bash
   sudo systemctl restart log-query-mcp.service
   ```

4. 重新执行 `list_log_sources` 和目标查询。

## 8. 升级和回滚

升级遵循 [安装指南](./INSTALL.md#10-升级)：

- 校验新包 SHA256。
- 备份当前二进制和配置。
- 停止服务。
- 安装新包。
- 启动并执行基础验证。

回滚遵循 [安装指南](./INSTALL.md#11-回滚)：

- 优先使用上一版本发布包。
- 回滚后执行 MCP 初始化、工具列表、真实查询和上下文读取。
- 记录回滚原因、版本、时间和验证结果。

## 9. 发布包校验

正式发布包应满足：

```bash
sha256sum -c SHA256SUMS
tar -tzf log-query-mcp-vVERSION-x86_64-unknown-linux-gnu.tar.gz
(cd log-query-mcp-vVERSION-x86_64-unknown-linux-gnu && sha256sum -c SHA256SUMS)
```

tag 必须与 `Cargo.toml` 的 `package.version` 一致。release workflow 会在 `v*` tag 上校验版本、构建 release binaries、运行 smoke tests、组装 tarball、生成 `BUILDINFO` 和 `SHA256SUMS`，并上传 GitHub Release artifacts。

## 10. 故障排查

| 现象 | 常见原因 | 处理 |
|---|---|---|
| 服务启动失败，提示 `LOG_QUERY_MCP_CONFIG is required` | systemd 环境变量缺失或 unit 被改坏 | 恢复 unit，执行 `systemctl daemon-reload` 后重启 |
| 服务启动失败，提示配置读取失败 | JSON 格式错误或字段不符合 schema | 用示例配置对比，修复后重启 |
| 查询返回 `UNKNOWN_SOURCE` | 客户端请求了未配置或禁用的 `source_id` | 执行 `list_log_sources`，改用返回的来源 ID |
| 查询返回 `SOURCE_UNAVAILABLE` | 文件缺失、权限不足或目录规则未发现文件 | 检查配置、文件存在性和 `log-query-mcp` 读取权限 |
| 查询返回 `FILE_CHANGED` | 搜索和上下文读取之间文件被替换或轮转 | 重试查询；必要时重启服务刷新快照 |
| 查询返回 `RESOURCE_LIMIT` | 结果、扫描字节、响应大小或上下文超限 | 缩小时间范围、关键词或来源；谨慎调整 limits |
| curl 返回连接失败 | 服务未启动或监听地址不同 | 检查 `systemctl status`、`journalctl`、`LOG_QUERY_MCP_BIND` |
| AI 客户端看不到工具 | 客户端 transport 或 URL 配置错误 | 使用 `type=streamable-http` 和 `/mcp` URL，先用 Inspector 验证 |

## 11. 变更记录建议

每次生产操作记录：

- 操作类型：安装、升级、回滚、配置变更、权限变更。
- 操作人和审批单。
- 版本号和包 SHA256。
- 配置摘要和涉及的 `source_id`。
- 验收结果。
- 回滚计划和实际回滚结果。
