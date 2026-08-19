# Log Query MCP 生产运维指南

本文面向负责运行 Log Query MCP 的运维和开发人员，覆盖 Local/Remote 日志来源、Direct SSH/SFTP Transport、监控、配置变更、SSH/SFTP、Cache、升级回滚和故障排查。ProxyCommand 运维内容保留为 post-v2 延后参考，不属于 v2.0 发布门禁。

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

HTTP endpoint 固定为 `/mcp`。当前没有独立 `/health` endpoint；发布包使用 `scripts/healthcheck.sh` 同时验证 systemd 状态和 MCP `initialize` 协议响应。

Remote SSH connection 有两种底层连接方式：

```text
Direct       : TcpStream → russh → auth → SFTP
ProxyCommand : admin program/argv → stdin/stdout raw stream → russh → auth → SFTP
```

ProxyCommand 只改变 SSH 底层字节流，不改变 host identity、credential、SFTP、Sync、Cache 或 Query Engine。

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

标准健康检查：

```bash
sudo scripts/healthcheck.sh
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

变更后重启并验证：

```bash
sudo systemctl restart log-query-mcp.service
sudo scripts/healthcheck.sh
```

每次配置变更至少记录：变更人、时间、source/connection 变化、Direct/ProxyCommand 变化、Secret/known_hosts 变化、权限变化、cache limit 变化、验证结果和回滚方案。

ProxyCommand 变更还必须记录：helper 程序来源/版本、程序路径、argv 模板、为何 Direct 不适用、目标网络路径和服务身份下的执行验证；这些记录只适用于未来重新纳入 ProxyCommand 的版本。

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
- ProxyCommand 只允许管理员配置的 `program + argv[]`，不经过 Shell。
- ProxyCommand 只允许 whole-argv `{host}` / `{port}` placeholder，不允许 credential/path/client parameter placeholder。
- Proxy helper stdout 是 SSH raw byte stream；不得混入 banner/debug/JSON 文本。
- Proxy helper 不获得 SSH Secret；认证仍由 russh 层执行。

systemd unit 的基础加固包括 `NoNewPrivileges=true`、`PrivateTmp=true`、`ProtectSystem=strict`、`ProtectHome=true` 和受限 address families。不要为了运行 Proxy helper 直接关闭全部 hardening；如目标 WSL/helper 与某一项冲突，应记录证据并做最小调整。

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

无论 Direct 还是 ProxyCommand，host-key identity 都是配置中的逻辑 `host:port`，不是 Proxy helper、Windows 主机或 localhost。

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

Private key 模式应把 key 文件权限限制给服务账户读取；加密私钥的 passphrase 仍通过 Secret reference 提供。ProxyCommand 不参与 Secret resolution。

### 5.4 ProxyCommand / WSL 运维（post-v2 延后）

本节不属于 v2.0 运维验收。v2.0 只验收 Direct SSH；ProxyCommand 代码和诊断路径保留，待 post-v2 重新授权并完成独立 live/performance/WSL 验收后再作为发布能力使用。

典型路径：

```text
WSL log-query-mcp
  → Windows helper executable
  → Windows/VPN network stack
  → logical SSH target
```

操作约束：

- 优先使用明确的管理员批准程序路径；不要依赖交互式 shell 的 PATH。
- 以 `log-query-mcp` 服务身份验证 helper，而不是只用管理员 shell 验证。
- 不把 `sh -c`、`bash -c`、`powershell -Command` 作为通用逃生口。
- helper stdout 必须保持纯字节流；诊断走 stderr。
- helper 退出/EOF/timeout/cancellation 后应被回收，不能长期残留孤儿进程。
- 全局 `max_concurrent_ssh_connections` 同时约束 Direct 与 Proxy session，Proxy child 不拥有独立绕过额度。

内部 Transport 分类用于运维定位：

```text
ProxyCommandNotFound
ProxyCommandPermissionDenied
ProxyCommandStartFailed
ProxyCommandStreamFailed
ProxyCommandTimeout
```

这些分类不得把完整 argv、raw stderr、Secret、private-key path 或 OS error 原样返回给 AI。上层 MCP 错误仍按稳定、去敏、fail-closed 模型处理。

WSL 排查顺序：

1. 确认 Direct path 的确不可达；如果 Direct 可达，优先保持 Direct 简单路径。
2. 以服务身份确认 helper 文件存在、可执行。
3. 确认 Windows executable interop/systemd 环境可启动 helper。
4. 确认 helper 使用宿主机/VPN 网络可到逻辑目标 `host:port`。
5. 确认 known_hosts 对逻辑目标正确，不能因为 Proxy 连接成功就跳过 host-key 验证。
6. 确认 SSH credential 只由服务 Secret/key 配置提供。
7. 检查 helper 是否异常退出、超时或残留进程。
8. 恢复后执行已知 `search_logs` 与 `get_log_context` smoke。

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

同步失败必须保持最后有效 cache，不允许部分同步覆盖有效 generation。Remote 默认 `allow_stale_on_error=false`，因此 SSH/认证/host-key/ProxyCommand 失败时会显式返回错误，而不是静默查询旧 cache。

Tail/FromNow 覆盖不足时返回 `CACHE_SCOPE_EXCEEDED`；这不是“没有结果”，而是“当前 cache 不能证明完整查询范围”。

ProxyCommand restart/failure harness 还要求：最后有效 generation 可保留用于恢复，但远端不可用时不能伪装为成功 stale query；恢复连接后再继续同步。

## 8. 运行健康检查

标准命令：

```bash
sudo scripts/healthcheck.sh
```

它要求：

```text
systemd active
    +
POST /mcp initialize 成功
    +
jsonrpc=2.0
    +
serverInfo.name = log-query-mcp
    +
无 JSON-RPC error
```

因此“进程仍在运行”不能单独证明服务健康。

如 endpoint 不是默认值：

```bash
sudo LOG_QUERY_MCP_URL=http://127.0.0.1:9000/mcp scripts/healthcheck.sh
```

`LOG_QUERY_MCP_HEALTHCHECK_SKIP_SYSTEMD=1` 仅用于明确的容器/测试场景，不作为普通 systemd 生产部署的绕过方式。

功能验证：

1. `list_log_sources`：确认只返回批准来源且不泄露 host/username/secret/cache path/ProxyCommand argv。
2. `search_logs`：Local、Direct Remote、Proxy Remote 使用已知 trace/request ID 验证。
3. `get_log_context`：用 `match_ref` 验证有限上下文与 pinned cache generation。
4. Proxy source 场景确认服务端没有执行 Shell/命令。

## 9. 监控建议

最低监控项：

- systemd 服务/端口状态。
- 周期性 MCP protocol initialize 外部探测。
- 启动和配置失败。
- `REMOTE_UNAVAILABLE` / auth / host-key / timeout 频率。
- ProxyCommand start/stream/timeout 类故障频率。
- Proxy helper 异常残留进程。
- `CACHE_SCOPE_EXCEEDED` / cache quota 频率。
- cache 磁盘空间。
- 查询 timeout/resource-limit 频率。
- 目标服务器 host key / 日志权限 / rotation 策略变化。
- WSL 场景的 Windows/VPN/interop 变化。

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
6. 执行标准 service + MCP protocol health check；
7. post-mutation/restart/protocol health 失败时自动调用 rollback。

升级前仍应保留上一版本 release artifact，并记录 backup path。如果生产配置包含 ProxyCommand，升级后额外执行 Proxy source smoke 和 helper cleanup 检查。

## 11. 回滚

显式回滚：

```bash
sudo scripts/rollback.sh /var/lib/log-query-mcp/backups/<backup-dir>
```

Rollback 恢复 upgrade 前的 binaries、BUILDINFO、config 和 systemd unit，然后 restart，并要求恢复后的 MCP protocol health check 成功。

如果自动 rollback 也失败，**不要继续反复执行 upgrade**。保留 backup，检查磁盘、权限、systemd 和 config，再人工恢复。

## 12. 发布包验证与 RC 检查

```bash
sha256sum -c SHA256SUMS
tar -tzf log-query-mcp-vVERSION-x86_64-unknown-linux-gnu.tar.gz
bash scripts/validate_release_package.sh \
  log-query-mcp-vVERSION-x86_64-unknown-linux-gnu.tar.gz \
  SHA256SUMS
```

v2.0 Package validator 现在要求：

- 包含 `schemas/log-query-mcp-config-v2.schema.json`；
- v2 example 只包含 Direct SSH connection；
- 包含 INSTALL/OPERATIONS/PRODUCTION_CHECKLIST 与 M6 基线文档。

本地仓库 Final Candidate 检查：

```bash
bash scripts/rc_check.sh
```

它覆盖所有非 live-SSH 的 v2.0 仓库 Gate，并显式检查 Direct-only release contract。真实 Direct SSH、Direct performance 和目标生产验收仍需单独执行；ProxyCommand functional/performance/WSL workflow 属于 post-v2 延后验证。

Release workflow 在 tag 上验证 tag/version，构建 release binaries，运行 transport smoke、protocol health 负向矩阵、upgrade/rollback 演练，组装并验证 tarball。普通 branch/PR package job 只有 `contents: read`；只有 tag publish job 获得 `contents: write`。

Rust、Contracts、SSH Transport、Direct performance、Release 均需要当前 candidate 的真实执行证据。当前 Actions runner 启动被 GitHub Billing/Spending Limit 外部阻塞，跟踪于 Issue #23；ProxyCommand workflow 不再是 v2.0 required gate。

## 13. Remote / ProxyCommand 故障排查

| 现象 | 常见原因 | 处理 |
|---|---|---|
| `REMOTE_UNAVAILABLE` | 网络、sshd、路由、Proxy helper 或目标服务不可达 | 按 Direct/Proxy 路径分层检查；不要改成 remote shell 工具 |
| Proxy program not found | program 路径/PATH 与服务身份不同 | 使用管理员批准的明确路径，并以服务身份验证 |
| Proxy permission denied/start failed | executable 权限、WSL interop、systemd hardening/环境 | 最小化修复执行条件，不关闭整体安全边界 |
| Proxy stream failed/early EOF | helper 崩溃、宿主机网络/VPN中断、stdout 被污染 | 检查 helper 生命周期和宿主机路径；不得把 stderr 直接回传 AI |
| Proxy timeout | helper 未连上目标或字节流未完成 SSH handshake | 验证逻辑 host:port 与宿主机/VPN 可达性 |
| auth failure | Secret/key/远端账号权限错误 | 核对 secret_ref、key 权限、账号状态；不要在日志打印 Secret |
| host key verification failure | known_hosts 缺失、目标 host key 变化 | 先独立核验逻辑目标 fingerprint，再更新 known_hosts |
| `CACHE_SCOPE_EXCEEDED` | Tail/FromNow cache 不覆盖请求历史范围 | 缩小查询范围或由管理员调整 bootstrap；不要当作无匹配 |
| cache limit | global/per-source quota 或 pinned generation 占用 | 检查 retention/active refs/磁盘，再调整容量 |
| rotation 后结果变化 | 新 generation 已创建 | 新查询读新 generation；旧 match_ref 在 TTL 内仍读旧 snapshot |
| Remote 一台失败 | 单 connection/Proxy 故障 | 其他独立 server/source 应保持可用；按 source 缩小排查 |
| systemd active 但 healthcheck 失败 | MCP endpoint/协议初始化异常 | 查 journal、配置、bind；不得把仅进程存活当成健康 |

通用错误仍遵守稳定 code + 去敏 message + retryable，不应返回绝对 remote/cache path、Secret、ProxyCommand 完整 argv/raw stderr、backtrace 或底层系统调用详情。

## 14. 性能基线

M6 历史 Direct 大文件基线：

- 100 MiB full cold bootstrap：约 4.9s（特定 GitHub Runner）。
- 1 GiB full cold bootstrap：约 48.6s。
- 10 GiB logical Tail(64 MiB)：约 3.1s，仅传约 64 MiB payload + bounded probe。
- unchanged probe：64 KiB。
- local cache scan：0 remote bytes。
- 300 次连续 range read：已验证 SFTP handle 确定关闭。

M7 已实现 paired Direct/Proxy performance harness。v2.0 只要求当前候选 Direct 性能证据；Proxy 300 range reads、混合并发和 helper cleanup 保留为 post-v2 非阻塞回归项。当前 Billing 阻塞导致对应 workflow `steps=null`，不能把它当作 PASS。

这些数字只用于回归对比，不是 SLA。详见 `M6_PERFORMANCE_BASELINE_V2.md` 与 `M7_PROXY_PERFORMANCE_GATE_V2.md`。

## 15. 变更记录

每次生产操作记录：操作类型、操作人/审批单、版本和 SHA256、配置/source/connection 摘要、Direct/ProxyCommand 选择及原因、helper 版本/路径（如适用）、known_hosts/Secret 变更、backup path、验收结果、遗留风险和回滚结果。
