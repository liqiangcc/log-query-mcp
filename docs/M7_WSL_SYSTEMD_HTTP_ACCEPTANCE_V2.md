# Log Query MCP v2 M7 WSL systemd HTTP Acceptance

> 状态：Tooling implemented / real target evidence pending  
> 日期：2026-08-10  
> Draft PR：#25  
> 客户端：[`scripts/m7_wsl_http_acceptance.py`](../scripts/m7_wsl_http_acceptance.py)  
> 上游 WSL 验收：[`M7_WSL_ACCEPTANCE_V2.md`](./M7_WSL_ACCEPTANCE_V2.md)

## 1. 目标

本 Gate 专门证明 **实际 systemd 服务进程** 可以在 WSL 中通过 Windows `.exe` ProxyCommand 访问目标 SSH/SFTP 日志。

它解决 stdio acceptance 无法证明的最后一个运行时差异：

```text
interactive/service-user WSL process
可能可以启动 Windows .exe

但

systemd log-query-mcp.service
未必拥有相同 Windows interop 环境
```

因此真实 M7 WSL 验收必须同时具备：

```text
service-identity stdio acceptance
+
actual systemd Streamable HTTP Proxy source acceptance
```

任何一个都不能替代另一个。

## 2. 关注分离

`scripts/m7_wsl_http_acceptance.py` 不启动新的 MCP server，也不修改系统配置。

它只负责：

1. 读取管理员现有 v2 config，定位指定 ProxyCommand source；
2. 使用 `systemctl show` 确认生产 service：
   - `ActiveState=active`；
   - `User` 等于预期服务身份；
   - `MainPID > 0`；
3. 对 `/proc/<MainPID>/exe` 与指定 installed candidate binary 做 SHA256 一致性校验；
4. 记录 Windows helper 进程基线；
5. 直接调用生产 `/mcp` endpoint；
6. 验证三个 AI-facing tools；
7. 记录 helper 是否回到基线；
8. 输出去敏 JSON evidence。

它不提供：

```text
remote exec
shell
write/upload/delete
任意 remote path
Secret 注入
service restart
systemd 配置修改
```

## 3. 为什么 `healthcheck.sh` 不够

标准 `scripts/healthcheck.sh` 只执行 MCP `initialize`。

这可以证明：

```text
systemd active
+
HTTP endpoint alive
+
MCP initialize works
```

但 `initialize` 不访问任何 Remote Source，因此不会触发：

```text
ProxyCommand
SSH handshake
known_hosts
Authentication
SFTP
Sync
Cache refresh
```

所以必须另外执行本 Gate。

## 4. 静态预检

仓库 CI / package validator 只执行：

```bash
python3 scripts/m7_wsl_http_acceptance.py \
  --validate-config-only \
  --config examples/log-query-mcp.v2.remote.json \
  --source-id inventory-remote-via-host
```

该模式仅检查配置结构，不调用：

```text
systemctl
/proc/<pid>/exe
Windows tasklist
网络
MCP endpoint
ProxyCommand
```

静态预检成功不能标记 real systemd acceptance PASS。

## 5. 真实执行前提

目标 WSL 环境必须已经：

- 安装待验收 candidate；
- 使用实际 `/etc/log-query-mcp/config.json`；
- 配置 strict known_hosts；
- 配置正常 SecretResolver 来源；
- `log-query-mcp.service` 已启动；
- selected source 使用 `proxy.type=command`；
- Proxy helper 是管理员批准的 Windows `.exe`；
- 目标日志存在一个可识别 acceptance marker；
- `tasklist.exe` 可从 WSL 调用。

本脚本不会创建 marker，也不会为了验收增加远程写权限。

## 6. 推荐执行顺序

先运行普通生产 health check：

```bash
sudo scripts/healthcheck.sh
```

再执行真正的 Proxy source Gate：

```bash
python3 scripts/m7_wsl_http_acceptance.py \
  --config /etc/log-query-mcp/config.json \
  --source-id inventory-remote-via-host \
  --keyword 'M7_WSL_ACCEPTANCE_MARKER_20260810' \
  --url http://127.0.0.1:8000/mcp \
  --service-name log-query-mcp.service \
  --expected-service-user log-query-mcp \
  --expected-http-bin /opt/log-query-mcp/bin/log-query-mcp \
  --buildinfo /opt/log-query-mcp/BUILDINFO \
  --evidence-dir /var/lib/log-query-mcp/m7-wsl-evidence
```

Secret 值不应放入命令行；它们应已经由生产 service 的既有 SecretResolver/systemd 环境提供。

## 7. Candidate Binary Identity

只看到 `MainPID` 不足以证明 systemd 正在运行当前候选版本。

因此脚本会读取：

```text
/proc/<MainPID>/exe
```

并分别计算：

```text
running process executable SHA256
expected installed HTTP binary SHA256
```

二者必须完全一致，否则返回：

```text
SERVICE_BINARY_MISMATCH
```

若无法读取或 hash，返回：

```text
SERVICE_BINARY_UNREADABLE
```

这避免升级后旧进程仍驻留、验收却错误归属于新 candidate 的情况。

## 8. MCP 证据链

客户端直接请求实际 production endpoint：

```text
initialize
notifications/initialized
tools/list
list_log_sources
search_logs
get_log_context
```

要求工具集合严格等于：

```text
list_log_sources
search_logs
get_log_context
```

`search_logs` 必须：

- 只指定 acceptance Proxy source；
- 找到指定 marker；
- 返回有效 `match_ref`。

随后 `get_log_context` 必须通过该 `match_ref` 返回包含 marker 的上下文。

因此成功的 `search_logs` 同时证明实际 systemd 进程完成：

```text
spawn Windows helper
→ raw SSH stream
→ strict known_hosts
→ SSH auth
→ read-only SFTP
→ Sync
→ Cache
→ Query
→ MCP response
```

## 9. HTTP Transport 兼容性

当前生产 binary 使用 Streamable HTTP：

```text
with_stateful_mode(false)
with_json_response(true)
```

验收客户端以 JSON POST 为主，同时：

- 要求 `initialize` 协商到当前 `2025-06-18` protocol；
- 后续请求携带 `MCP-Protocol-Version`；
- 若返回 `Mcp-Session-Id`，后续请求会自动携带；
- Accept 同时允许 `application/json` 与 `text/event-stream`；
- 对 SSE 响应可解析 `data:` JSON；
- 非 UTF-8 body fail-closed；
- 单次 HTTP response 设 8 MiB acceptance 上限，避免故障情况下无界读取。

这只是验收客户端兼容性，不改变 MCP server transport contract。

## 10. Service Identity 证明

脚本默认要求：

```text
ActiveState=active
User=log-query-mcp
MainPID>0
```

如果部署使用其他服务账户，可通过：

```bash
--expected-service-user custom-service-user
```

显式指定。

如果 systemd `User` 不匹配，必须返回：

```text
SERVICE_IDENTITY_MISMATCH
```

不能用当前 shell 用户成功来替代生产服务身份。

## 11. Helper 生命周期

开始 MCP 调用前，脚本通过 Windows：

```text
tasklist.exe
```

记录 helper image 的进程数量 `N`。

三工具调用结束后等待 helper 回收，并要求：

```text
after_count <= N
```

如果持续高于基线，返回：

```text
HELPER_PROCESS_LEAK
```

系统已有其他同名 helper 不要求归零，本 Gate 只证明本次生产查询没有新增残留进程。

## 12. Evidence

成功或失败都会尽可能写入 `0600` JSON，例如：

```text
m7-wsl-evidence/m7-wsl-http-acceptance-20260810T120000Z.json
```

记录：

- config SHA256；
- BUILDINFO version/target/git_commit；
- source_id / connection_id；
- logical host SHA256；
- target port；
- auth type；
- helper basename；
- Proxy argv shape；
- keyword SHA256；
- endpoint SHA256；
- systemd ActiveState/User/MainPID；
- running process executable SHA256；
- expected candidate HTTP binary SHA256；
- initialize/tools-list/三工具 PASS 状态；
- search result count；
- context line count；
- helper before/after count；
- 最终 PASS/FAIL 和稳定 failure category。

明确不保存：

```text
Secret value
password
private key/passphrase
logical host plaintext
marker plaintext
log content
match_ref
raw stderr
完整底层 OS error
```

## 13. 典型失败分类

```text
CONFIG_UNREADABLE
SOURCE_NOT_FOUND
PROXY_NOT_COMMAND
HELPER_NOT_WINDOWS_EXE
KNOWN_HOSTS_UNREADABLE
SYSTEMCTL_UNAVAILABLE
SERVICE_NOT_ACTIVE
SERVICE_IDENTITY_MISMATCH
SERVICE_PID_INVALID
EXPECTED_BINARY_UNREADABLE
SERVICE_BINARY_UNREADABLE
SERVICE_BINARY_MISMATCH
TASKLIST_UNAVAILABLE
HTTP_TRANSPORT_ERROR
HTTP_STATUS_ERROR
HTTP_BODY_ENCODING_INVALID
MCP_PROTOCOL_INVALID
MCP_JSONRPC_ERROR
TOOLS_SURFACE_CHANGED
SOURCE_NOT_LISTED
SEARCH_NO_MATCH
CONTEXT_MARKER_MISSING
HELPER_PROCESS_LEAK
```

这些是 acceptance-local 分类，不是新的 MCP public error contract。

## 14. Final Gate

真实目标 evidence 必须满足：

- [ ] `healthcheck.sh` PASS；
- [ ] systemd ActiveState/User/MainPID PASS；
- [ ] running process binary SHA256 与 expected candidate binary 一致；
- [ ] `tools/list` 只有三个工具；
- [ ] `list_log_sources` 包含 acceptance Proxy source；
- [ ] `search_logs` 通过 production systemd service 找到 marker；
- [ ] `get_log_context` 返回 marker context；
- [ ] helper count 回到 baseline；
- [ ] evidence JSON 已保存；
- [ ] 未发生 Secret/raw stderr/log-content 泄漏。

当前仓库只能标记：

```text
systemd HTTP acceptance tooling  IMPLEMENTED
real target evidence             PENDING
```

在真实 evidence 与所有 Rust / Contracts / Direct SSH / M7 / Performance / Release gates 同时通过前，PR #25 必须保持 Draft。