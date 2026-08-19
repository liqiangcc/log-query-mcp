# Log Query MCP v2 M7 WSL Acceptance

> 状态：Acceptance procedure/tooling implemented / real target evidence pending  
> 日期：2026-08-10  
> Draft PR：#25  
> Service-identity wrapper：[`scripts/m7_wsl_acceptance.sh`](../scripts/m7_wsl_acceptance.sh)  
> MCP acceptance client：[`scripts/m7_wsl_acceptance.py`](../scripts/m7_wsl_acceptance.py)

## 1. 目标

本验收用于证明 M7 ProxyCommand 的真实目标场景，而不是重复 Linux CI：

```text
WSL 中的 log-query-mcp
  ↓
WSL 直接到目标 SSH 不可达
  ↓
管理员配置的 Windows .exe ProxyCommand helper
  ↓ Windows 网络栈 / VPN / 企业网络
目标 SSH Server
  ↓
strict known_hosts + SSH auth + read-only SFTP
  ↓
Sync / Cache / Query
  ↓
list_log_sources / search_logs / get_log_context
```

只有在真实 Windows + WSL + 目标网络环境执行并保存证据后，`WSL acceptance` 才能标记 PASS。GitHub Hosted Runner、普通 Linux VM、仅 schema/contract 检查都不能替代本验收。

## 2. 冻结边界

本验收不改变产品权限模型：

- ProxyCommand 仍只提供 SSH raw byte stream；
- 不增加 MCP tool；
- 不允许 `ssh_exec`、Shell、远端命令、上传、写、删除、部署或任意 remote path；
- Host Key Verification 仍以配置中的逻辑 `host` / `port` 为目标；
- Password/private-key/passphrase 仍由 SSH / SecretResolver 层处理，不传给 ProxyCommand；
- helper stdout 仍只能是 SSH 协议字节流；
- helper stderr 不进入 AI-facing 返回；
- `allow_stale_on_error=false` 继续 fail-closed。

`scripts/m7_wsl_acceptance.py` 不会把 Secret、完整日志内容、`match_ref` 或逻辑 host 明文写进 evidence JSON。

## 3. 前置条件

真实验收机器必须满足：

1. Windows 上存在目标企业网络/VPN访问能力；
2. WSL 可执行 Windows `.exe`；
3. `log-query-mcp-stdio` 为待验收 candidate binary；
4. v2 config 中选择一个 `backend.type=ssh` 的 ProxyCommand source；
5. 对应 connection 配置：
   - `proxy.type=command`；
   - `program` 为 Windows `.exe`，例如 `ncat.exe` 或受控绝对路径；
   - `args` 至少包含完整 argv 项 `{host}` 与 `{port}`；
   - strict `known_hosts_file`；
   - 正常 SSH password 或 private-key auth；
6. Windows `tasklist.exe` 可从 WSL 调用，用于 helper 生命周期证据；
7. 目标日志中存在一个可安全用于验收的唯一 marker；
8. 最终 acceptance 必须由实际服务身份执行，默认是 `log-query-mcp`。

marker 应通过正常业务/测试路径产生，或使用已存在的可识别测试日志。不要为了制造 marker 给 Log Query MCP 增加远程写/执行能力。

## 4. 推荐配置形态

示意：

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

仓库示例：`examples/log-query-mcp.v2.remote.json` 中的 `inventory-remote-via-host`。

正式验收应使用实际目标配置，不要把示例 host、Secret reference 或目录直接当生产值。

## 5. known_hosts

ProxyCommand 只改变网络路径，不改变服务器身份。

必须先通过独立可信渠道核验目标 SSH host key fingerprint，再安装 `known_hosts_file`。不要把 Windows helper、VPN gateway、localhost 或 WSL host 当成 SSH server identity。

验收脚本会确认 `known_hosts_file` 实际存在；真正 host-key 匹配仍由产品 SSH 层执行。若 host key 不匹配，搜索必须 fail-closed。

## 6. 静态预检

在任何 Linux/CI 环境都可以执行：

```bash
python3 scripts/m7_wsl_acceptance.py \
  --validate-config-only \
  --config examples/log-query-mcp.v2.remote.json \
  --source-id inventory-remote-via-host
```

它只检查：

- config `version=2`；
- selected source 使用 SSH backend；
- connection 存在；
- `proxy.type=command`；
- program/args 结构；
- placeholder 只允许完整 `{host}` / `{port}`；
- acceptance source 同时具有 `{host}` 与 `{port}`；
- auth/known_hosts contract 存在。

该模式不检测 WSL、不访问网络、不启动 helper，也不代表真实 acceptance PASS。

`rc_check.sh` 和 release-package validator 只运行这一 static mode。

## 7. Service Identity Gate

真实验收不要直接调用 Python client，而应通过：

```text
scripts/m7_wsl_acceptance.sh
```

wrapper 默认要求：

```text
id -un == log-query-mcp
```

否则返回：

```text
SERVICE_IDENTITY_MISMATCH
```

如果生产 service 使用不同的受控用户，可以显式设置：

```bash
M7_WSL_EXPECTED_USER=custom-service-user
```

`M7_WSL_ALLOW_USER_MISMATCH=1` 只用于本地诊断；使用该绕过产生的结果不能作为 Final WSL Acceptance。

这个 gate 很重要：Windows interop / helper execution 不能只在管理员自己的交互用户下成功，必须由真正运行 Log Query MCP 的身份成功。

## 8. 真实 WSL stdio 验收

先确保 candidate Secret 已通过环境/systemd 等既有方式提供。不要把明文 Secret 写进命令行。

推荐从 release 解包目录，以实际服务身份执行。示意：

```bash
sudo --preserve-env=LOG_QUERY_MCP_INVENTORY_PASSWORD \
  -u log-query-mcp \
  ./scripts/m7_wsl_acceptance.sh \
  --config /etc/log-query-mcp/config.json \
  --source-id inventory-remote-via-host \
  --keyword 'M7_WSL_ACCEPTANCE_MARKER_20260810' \
  --stdio-bin /opt/log-query-mcp/bin/log-query-mcp-stdio \
  --buildinfo /opt/log-query-mcp/BUILDINFO \
  --evidence-dir /var/lib/log-query-mcp/m7-wsl-evidence
```

实际 Secret environment 名称按生产配置使用。不要在 shell history、ticket 或 evidence 中写 Secret 值。

默认 acceptance 要求：

```text
actual user = configured service identity       PASS
WSL detected                                    PASS
selected source is ProxyCommand-backed          PASS
proxy helper is Windows .exe                    PASS
known_hosts file exists                         PASS
WSL direct TCP to logical SSH target             FAIL as expected
Windows helper process baseline captured        PASS
MCP initialize                                  PASS
tools/list exactly three tools                  PASS
list_log_sources contains selected source       PASS
search_logs finds acceptance marker             PASS
get_log_context returns marker context          PASS
helper process count returns to baseline        PASS
```

如果 WSL 本身已经可以直接连接目标 SSH，Python client 默认失败为 `DIRECT_PATH_REACHABLE`。这不代表 ProxyCommand 功能坏了，而是该次运行没有证明“WSL 不可达但 Windows Host 可达”的目标网络场景。

`--allow-direct-reachable` 只用于辅助诊断，不应作为最终 M7 WSL-path acceptance 证据。

## 9. 三个 MCP 工具如何被证明

client 使用待验收的 `log-query-mcp-stdio` 和同一个 v2 config，执行标准 MCP sequence：

```text
initialize
notifications/initialized
tools/list

tools/call list_log_sources
tools/call search_logs
tools/call get_log_context
```

要求 `tools/list` 的工具集合严格等于：

```text
list_log_sources
search_logs
get_log_context
```

`search_logs` 必须通过指定 Proxy source 找到 marker，并产生 `match_ref`；随后 `get_log_context` 必须使用该 `match_ref` 读取包含 marker 的上下文。

因为该 source 的 backend 指向 ProxyCommand SSH connection，所以成功的 `search_logs` 同时证明：

```text
Windows helper byte stream
+ SSH handshake
+ strict host key
+ configured auth
+ read-only SFTP
+ Sync
+ Cache
+ Query
```

形成完整链路。

## 10. systemd / Streamable HTTP 必须单独证明

stdio acceptance 证明“service identity + candidate binary + v2 config + Windows helper + MCP tools”的确定性链路，但 **不能替代真实 systemd service**。

WSL Windows interop 尤其需要注意：交互 shell 中存在的 interop 环境不代表 systemd service 一定拥有同样的 Windows executable 启动条件。

因此最终验收还必须对实际运行的 systemd service 执行：

```bash
sudo scripts/healthcheck.sh
```

随后通过真实 AI client / MCP Inspector 对**同一个 ProxyCommand source**至少完成：

```text
list_log_sources
search_logs(acceptance marker)
get_log_context(match_ref)
```

其中 `search_logs` 成功是关键：单纯 `initialize` healthcheck 不会触发 SSH/ProxyCommand，不能证明 systemd service 可以启动 Windows helper。

因此最终证据组合必须是：

```text
service-identity stdio WSL acceptance JSON
+
production systemd HTTP healthcheck PASS
+
production systemd Proxy source three-tool smoke PASS
```

## 11. Helper 生命周期

真实验收前 client 通过 Windows `tasklist.exe` 记录 helper image 的进程数量，例如：

```text
ncat.exe count = N
```

MCP 工具调用结束并终止 acceptance stdio server 后，client 等待 helper 回收，并要求：

```text
after_count <= before_count
```

这样即使系统本来已有无关 `ncat.exe`，也不会要求全局数量必须为 0；只要求本次验收不能增加残留 helper。

若最终数量高于基线，状态必须为 `HELPER_PROCESS_LEAK`，不能人工改成 PASS。

生产 systemd smoke 后也应再次检查 helper 数量没有持续增长。

## 12. Evidence JSON

成功或失败都会生成 `0600` JSON，例如：

```text
/var/lib/log-query-mcp/m7-wsl-evidence/m7-wsl-acceptance-20260810T120000Z.json
```

Evidence 记录：

- config SHA256；
- candidate stdio binary SHA256；
- BUILDINFO 中可用的 version/target/git_commit；
- source_id / connection_id；
- logical target host 的 SHA256，而不是明文 host；
- target port；
- auth type；
- ProxyCommand program basename / argv shape；
- marker keyword SHA256，而不是 marker 明文；
- WSL distro/kernel；
- Direct TCP 是否不可达；
- 三个 MCP 工具的 PASS 状态；
- search result count / context line count，不保存日志正文；
- helper process before/after count；
- 最终 PASS/FAIL 和稳定 failure category。

不保存：

```text
password
private key / passphrase
SecretResolver value
raw server stderr
log line content
match_ref
logical target host plaintext
```

systemd HTTP smoke 的人工记录也应遵守相同去敏原则。

## 13. Failure categories

Acceptance tooling 只输出稳定、安全的本地分类，例如：

```text
SERVICE_IDENTITY_MISMATCH
NOT_WSL
DIRECT_PATH_REACHABLE
TASKLIST_UNAVAILABLE
HELPER_NOT_WINDOWS_EXE
KNOWN_HOSTS_UNREADABLE
MCP_RESPONSE_TIMEOUT
MCP_TOOL_ERROR
SEARCH_NO_MATCH
CONTEXT_MARKER_MISSING
HELPER_PROCESS_LEAK
```

它不会把 raw ProxyCommand stderr、Secret 或完整日志正文写入 evidence。

产品自身的 SSH/ProxyCommand 错误分类仍以代码中的 transport/tool error contract 为准；acceptance failure category 不是新的 MCP API。

## 14. Final Acceptance Checklist

真实目标环境必须逐项记录：

- [ ] candidate commit / binary SHA256 可追溯；
- [ ] config SHA256 可追溯；
- [ ] Windows/WSL 版本与 distro 已记录；
- [ ] VPN/企业网络处于目标生产等价状态；
- [ ] WSL Direct TCP 到 logical SSH target 确实不可达；
- [ ] acceptance wrapper 以实际 service identity 执行；
- [ ] Windows `.exe` helper 可由实际服务身份启动；
- [ ] strict known_hosts 已独立核验；
- [ ] configured SSH auth 成功；
- [ ] SFTP/Sync/Cache/Search 完整链路成功；
- [ ] stdio `list_log_sources` PASS；
- [ ] stdio `search_logs` PASS；
- [ ] stdio `get_log_context` PASS；
- [ ] helper count 回到基线；
- [ ] evidence JSON 已保存；
- [ ] `scripts/healthcheck.sh` 对生产 systemd HTTP service PASS；
- [ ] production systemd service 对同一 Proxy source 的三工具 smoke PASS；
- [ ] systemd smoke 后 helper 没有持续增长；
- [ ] 未出现 Secret/path/raw stderr 泄漏；
- [ ] 操作人、时间、目标环境和审批/测试记录可追溯。

## 15. RC 边界

仓库中存在本验收 tooling 和文档，只能标记：

```text
WSL acceptance procedure/tooling  IMPLEMENTED
real WSL evidence                 PENDING
systemd Proxy source evidence     PENDING
```

只有真实目标环境证据满足本清单，且当前 candidate 的 Rust / Contracts / Direct SSH / all M7 / Performance / Release gates 同时 PASS，PR #25 才能进入 Ready/merge 评估。
