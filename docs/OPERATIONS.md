# Log Query MCP 生产运维指南

本文面向负责运行 Log Query MCP 的运维和开发人员，覆盖 Local/Remote 日志来源、监控、配置变更、SSH/SFTP、Cache、升级回滚和故障排查。

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

HTTP endpoint 固定为 `/mcp`。当前没有单独 health endpoint，健康检查使用 systemd 状态、端口监听和 MCP initialize/tools 调用。

## 2. 常用命令

```bash
sudo systemctl status --no-pager log-query-mcp.service
sudo systemctl restart log-query-mcp.service
sudo systemctl stop log-query-mcp.service
sudo systemctl start log-query-mcp.service
journalctl -u log-query-mcp.service -f
journalctl -u log-query-mcp.service -n 200 --no-pager
ss -ltnp | grep ':8000'
```

确认构建信息：

```bash
cat /opt/log-query-mcp/BUILDINFO
sha256sum /opt/log-query-mcp/bin/log-query-mcp /opt/log-query-mcp/bin/log-query-mcp-stdio
```

## 3. 配置变更

编辑：

```bash
sudoedit /etc/log-query-mcp/config.json
```

变更后重启：

```bash
sudo systemctl restart log-query-mcp.service
sudo systemctl status --no-pager log-query-mcp.service
```

每次配置变更至少记录：变更人、时间、source/connection 变化、Secret/known_hosts 变化、权限变化、cache limit 变化、验证结果和回滚方案。

## 4. 安全边界

Log Query MCP 是只读日志查询服务，但日志内容本身可能敏感。生产必须遵守：

- 默认只监听 `127.0.0.1:8000`。
- 服务不内置客户端认证和 TLS；非 loopback 暴露必须通过受控网关/ACL/TLS。
- Local Source 只批准必要 root，不配置 `/`、整个 `/var` 或应用根目录。
- Remote Source 的 host、username、credential、root 只允许管理员配置，AI 请求不能提交。
- Remote Transport 只使用 SFTP 只读操作，不提供 Remote Exec、Shell、upload、write、delete、deploy/restart。
- Password/private-key passphrase 使用 `secret_ref`，不得把明文 Secret 放进普通配置或日志。
- Host Key Verification 必须 fail-closed。
- Remote 账号推荐专用 `log-reader`：无 sudo、只读日志权限，条件允许时使用 SFTP-only/chroot。

systemd unit 的基础加固包括 `NoNewPrivileges=true`、`PrivateTmp=true`、`ProtectSystem=strict`、`ProtectHome=true` 和受限 address families。不要为了省事整体放宽这些边界。

## 5. Remote Source 运维

### 5.1 known_hosts 初始化

在 MCP 主机上由管理员获取并核验目标服务器 host key，再写入受控 known_hosts。示例：

```bash
ssh-keyscan -t ed25519 -p 22 server.example.internal > /etc/log-query-mcp/known_hosts.new
ssh-keygen -lf /etc/log-query-mcp/known_hosts.new
```

**必须通过独立可信渠道核对 fingerprint**，不要把 `ssh-keyscan` 的网络结果直接视为可信身份。

确认后：

```bash
sudo install -m 0644 /etc/log-query-mcp/known_hosts.new /etc/log-query-mcp/known_hosts
```

### 5.2 Host key rotation

Host key 变化时服务应拒绝连接，而不是自动接受。处理步骤：

1. 确认是计划内 rotation，而不是中间人风险。
2. 从可信渠道获取新 fingerprint。
3. 更新 known_hosts。
4. 重启服务或重新查询。
5. 执行目标 Remote Source smoke query。

### 5.3 Secret

Password 示例：

```text
config: secret_ref=ORDER_LOG_PASSWORD
environment: ORDER_LOG_PASSWORD=<secret>
```

Private key 模式应把 key 文件权限限制给服务账户读取；加密私钥的 passphrase 仍通过 Secret reference 提供。

## 6. Cache 运维和容量

默认数据/cache root：

```text
/var/lib/log-query-mcp/cache
```

Cache 是内部数据库，不是远端路径镜像：目录/文件使用 opaque IDs，目录权限 0700、数据文件 0600，不保存 SSH credential。

容量规划至少考虑：

```text
per-source bootstrap range
+ expected append growth during retention window
+ multiple generations retained for active cursor/match_ref
+ safety margin
```

对于超大日志优先 Tail bootstrap；M6 基线证明 10 GiB logical file 可以只缓存 64 MiB tail，而不需要下载完整 10 GiB。不要把 benchmark 数字当 SLA，实际容量应根据业务日志增长率和查询窗口设置。

遇到 `CACHE_LIMIT_EXCEEDED`：

1. 查看 global/per-source cache limits。
2. 检查是否存在仍被 cursor/match_ref pin 的 generation。
3. 缩短 retention 或调整 bootstrap 范围前先确认查询需求。
4. 扩容前确认磁盘剩余空间和 inode。

不要手工删除 current/pinned generation。外部删除会 fail-closed，并可能导致正在使用的引用失效。

## 7. 日志轮转、替换和同步

Remote SyncEngine 使用 size + bounded continuity fingerprint 判断 append/replacement/truncate。正常 append 延续 generation；truncate/replacement/continuity mismatch 创建新 generation，旧 generation 在引用 TTL 内可继续读取。

同步失败必须保持最后有效 cache，不允许部分同步覆盖有效 generation。Remote 默认 `allow_stale_on_error=false`，因此 SSH/认证/host-key 失败时会显式返回错误，而不是静默查询旧 cache。

Tail/FromNow 覆盖不足时返回 `CACHE_SCOPE_EXCEEDED`；这不是“没有结果”，而是“当前 cache 不能证明完整查询范围”。

## 8. 运行健康检查

```bash
sudo systemctl is-active log-query-mcp.service
ss -ltn | grep '127.0.0.1:8000'
```

协议初始化：

```bash
curl -sS http://127.0.0.1:8000/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"ops-smoke","version":"0.1.0"}}}'
```

功能验证：

1. `list_log_sources`：确认只返回批准来源且不泄露 host/username/secret/cache path。
2. `search_logs`：Local 与 Remote 各使用已知 trace/request ID 验证。
3. `get_log_context`：用 `match_ref` 验证有限上下文。
4. Remote 场景确认服务端没有执行 Shell/命令。

## 9. 监控建议

最低监控项：

- systemd 服务/端口状态。
- 启动和配置失败。
- `REMOTE_UNAVAILABLE` / auth / host-key / timeout 频率。
- `CACHE_SCOPE_EXCEEDED` / cache quota 频率。
- cache 磁盘空间。
- 查询 timeout/resource-limit 频率。
- 目标服务器 host key / 日志权限 / rotation 策略变化。

当前未暴露 Prometheus metrics。需要指标时优先使用 journal + systemd + 外部探测，不要临时增加远程管理接口。

## 10. 安全升级

先验证 release 外层 checksum：

```bash
sha256sum -c SHA256SUMS
```

再执行：

```bash
sudo scripts/upgrade.sh /path/to/log-query-mcp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
```

`upgrade.sh` 会：

1. 校验包内 `SHA256SUMS`；
2. 备份 binaries、BUILDINFO、config、systemd unit 到 `/var/lib/log-query-mcp/backups`；
3. 保留现有生产 config；
4. 使用同目录 temporary file + fsync best-effort + rename 原子替换运行文件；
5. daemon-reload/restart；
6. 执行 health check；
7. post-mutation 失败时自动调用 rollback。

升级前仍应保留上一版本 release artifact，并记录 backup path。

## 11. 回滚

显式回滚：

```bash
sudo scripts/rollback.sh /var/lib/log-query-mcp/backups/<backup-dir>
```

Rollback 恢复 upgrade 前的 binaries、BUILDINFO、config 和 systemd unit，然后 restart + health check。

如果自动 rollback 也失败，**不要继续反复执行 upgrade**。保留 backup，检查磁盘、权限、systemd 和 config，再人工恢复。

## 12. 发布包验证

```bash
sha256sum -c SHA256SUMS
tar -tzf log-query-mcp-vVERSION-x86_64-unknown-linux-gnu.tar.gz
bash scripts/validate_release_package.sh \
  log-query-mcp-vVERSION-x86_64-unknown-linux-gnu.tar.gz \
  SHA256SUMS
```

Release workflow 在 tag 上验证 tag/version，构建 release binaries，运行 transport smoke、upgrade/rollback 演练，组装并验证 tarball。普通 branch/PR package job 只有 `contents: read`；只有 tag publish job 获得 `contents: write`。

## 13. Remote 故障排查

| 现象 | 常见原因 | 处理 |
|---|---|---|
| `REMOTE_UNAVAILABLE` | 网络、sshd、路由或目标服务不可达 | 从 MCP 主机检查 TCP/SSH 连通性，不要改成 remote shell 工具 |
| auth failure | Secret/key/远端账号权限错误 | 核对 secret_ref、key 权限、账号状态；不要在日志打印 Secret |
| host key verification failure | known_hosts 缺失、目标 host key 变化 | 先独立核验 fingerprint，再更新 known_hosts |
| `CACHE_SCOPE_EXCEEDED` | Tail/FromNow cache 不覆盖请求历史范围 | 缩小查询范围或由管理员调整 bootstrap；不要当作无匹配 |
| cache limit | global/per-source quota 或 pinned generation 占用 | 检查 retention/active refs/磁盘，再调整容量 |
| rotation 后结果变化 | 新 generation 已创建 | 新查询读新 generation；旧 match_ref 在 TTL 内仍读旧 snapshot |
| Remote 一台失败 | 单 connection 故障 | 其他独立 server/source 应保持可用；按 source 缩小排查 |

通用错误仍遵守稳定 code + 去敏 message + retryable，不应返回绝对 remote/cache path、Secret、backtrace 或底层系统调用详情。

## 14. 性能基线

当前 M6 大文件基线：

- 100 MiB full cold bootstrap：约 4.9s（特定 GitHub Runner）。
- 1 GiB full cold bootstrap：约 48.6s。
- 10 GiB logical Tail(64 MiB)：约 3.1s，仅传约 64 MiB payload + bounded probe。
- unchanged probe：64 KiB。
- local cache scan：0 remote bytes。
- 300 次连续 range read：已验证 SFTP handle 确定关闭。

这些数字只用于回归对比，不是 SLA。详见 `M6_PERFORMANCE_BASELINE_V2.md`。

## 15. 变更记录

每次生产操作记录：操作类型、操作人/审批单、版本和 SHA256、配置/source/connection 摘要、known_hosts/Secret 变更、backup path、验收结果、遗留风险和回滚结果。
