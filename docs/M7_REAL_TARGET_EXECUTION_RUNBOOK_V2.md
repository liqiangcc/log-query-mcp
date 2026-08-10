# Log Query MCP v2 M7 Real Target Execution Runbook

> 状态：Execution procedure implemented / real target execution pending  
> 日期：2026-08-10  
> Draft PR：#25

## 1. 目的

本 Runbook 是 M7 ProxyCommand 的真实目标执行入口。仓库中的 Linux CI、静态配置校验、synthetic evidence self-test 都不能替代本流程。

最终必须获得两份来自同一 candidate、同一配置、同一 ProxyCommand source、同一 acceptance marker 的真实证据：

```text
service-identity stdio evidence
+
production systemd HTTP evidence
```

然后使用 `scripts/verify_m7_evidence.py` 离线验证这两份证据的一致性与去敏契约。

## 2. 固定边界

真实验收不改变产品权限：

- ProxyCommand 只提供 SSH raw byte stream；
- 不增加 Shell / remote exec / upload / write / delete / deploy；
- strict known_hosts 仍绑定逻辑 SSH host/port；
- SecretResolver 继续负责 password/private-key passphrase；
- acceptance tooling 不创建远端 marker，不获得远程写权限；
- evidence 不保存 Secret、日志正文、match_ref、raw stderr 或 logical host 明文。

## 3. 验收前冻结 candidate

开始验收前记录：

```bash
cat /opt/log-query-mcp/BUILDINFO
sha256sum /opt/log-query-mcp/bin/log-query-mcp
sha256sum /opt/log-query-mcp/bin/log-query-mcp-stdio
sha256sum /etc/log-query-mcp/config.json
```

验收期间不要升级 binary、修改 config、切换 Proxy helper 或改变 selected source。若 candidate/config 发生变化，两份 evidence 必须全部重新执行。

## 4. Windows / 网络前置检查

确认：

1. Windows 已连接目标 VPN/企业网络；
2. Windows 上批准的 TCP helper 已安装，例如 `ncat.exe`；
3. WSL 可以调用 Windows executable：

```bash
tasklist.exe /? >/dev/null
```

4. selected Proxy connection 的 `program` 指向受控 Windows `.exe`；
5. `args` 使用完整 argv `{host}` / `{port}`；
6. 不把 password、key、passphrase 放进 Proxy argv。

## 5. WSL / systemd 前置检查

确认 WSL 使用 systemd，且生产服务存在：

```bash
systemctl show log-query-mcp.service \
  --property=ActiveState \
  --property=User \
  --property=MainPID
```

正式部署默认应看到：

```text
ActiveState=active
User=log-query-mcp
MainPID=<positive pid>
```

确认配置和 known_hosts 对实际 service identity 可读；cache 目录可写；Windows helper 可由该身份启动。

不要通过放宽成 root、关闭 systemd hardening 或关闭 known_hosts 来让验收通过。

## 6. SSH host key

通过独立可信渠道确认真实 SSH server host key fingerprint，再安装对应 known_hosts。

ProxyCommand 改变的是网络路径，不是 SSH identity。不得把以下对象写成目标 SSH identity：

```text
Windows host
VPN gateway
localhost
WSL host
Proxy helper
```

## 7. Acceptance marker

选择目标日志中已存在或通过正常测试/业务路径产生的唯一 marker，例如：

```text
M7_WSL_ACCEPTANCE_<timestamp-or-id>
```

不要让 Log Query MCP 自己写 marker。

建议在 shell 中只保存变量：

```bash
export M7_SOURCE_ID='<configured-proxy-source-id>'
export M7_ACCEPTANCE_MARKER='<known-marker>'
export M7_EVIDENCE_DIR='/var/lib/log-query-mcp/m7-wsl-evidence'
```

marker 不是 Secret，但最终 evidence 只保存其 SHA256。

## 8. Gate A：service-identity stdio

以真实 service identity 执行：

```bash
sudo --preserve-env \
  -u log-query-mcp \
  ./scripts/m7_wsl_acceptance.sh \
  --config /etc/log-query-mcp/config.json \
  --source-id "${M7_SOURCE_ID}" \
  --keyword "${M7_ACCEPTANCE_MARKER}" \
  --stdio-bin /opt/log-query-mcp/bin/log-query-mcp-stdio \
  --buildinfo /opt/log-query-mcp/BUILDINFO \
  --evidence-dir "${M7_EVIDENCE_DIR}"
```

该 Gate 必须证明：

```text
actual service identity                    PASS
real WSL                                   PASS
WSL Direct TCP to target                   unavailable
Windows helper path                        works
strict known_hosts                         works
SSH auth + read-only SFTP                  works
list_log_sources                           PASS
search_logs(marker)                        PASS
get_log_context(match_ref)                 PASS
helper count returns to baseline           PASS
```

失败时保留 FAIL evidence，不要手工修改成 PASS。

## 9. Gate B：production healthcheck

```bash
sudo ./scripts/healthcheck.sh
```

它只证明 systemd/HTTP/MCP initialize 健康，不证明 ProxyCommand。因此该步骤 PASS 后必须继续 Gate C。

## 10. Gate C：production systemd HTTP Proxy source

直接对实际 systemd service 执行：

```bash
python3 ./scripts/m7_wsl_http_acceptance.py \
  --config /etc/log-query-mcp/config.json \
  --source-id "${M7_SOURCE_ID}" \
  --keyword "${M7_ACCEPTANCE_MARKER}" \
  --url http://127.0.0.1:8000/mcp \
  --service-name log-query-mcp.service \
  --expected-service-user log-query-mcp \
  --expected-http-bin /opt/log-query-mcp/bin/log-query-mcp \
  --buildinfo /opt/log-query-mcp/BUILDINFO \
  --evidence-dir "${M7_EVIDENCE_DIR}"
```

该 Gate 必须证明：

```text
systemd ActiveState/User/MainPID           PASS
/proc/<MainPID>/exe candidate SHA256       PASS
initialize + tools/list                    PASS
list_log_sources                           PASS
search_logs(marker)                        PASS
get_log_context(match_ref)                 PASS
Windows helper cleanup                     PASS
```

## 11. 找到两份 evidence

```bash
ls -lt "${M7_EVIDENCE_DIR}"/m7-wsl-acceptance-*.json
ls -lt "${M7_EVIDENCE_DIR}"/m7-wsl-http-acceptance-*.json
```

选择本次执行产生的两份文件，不要混用不同 candidate、不同 config 或不同 marker 的历史 evidence。

例如：

```bash
STDIO_EVIDENCE='<m7-wsl-acceptance-file>'
HTTP_EVIDENCE='<m7-wsl-http-acceptance-file>'
```

## 12. Gate D：离线 evidence verifier

```bash
python3 ./scripts/verify_m7_evidence.py \
  --stdio-evidence "${STDIO_EVIDENCE}" \
  --http-evidence "${HTTP_EVIDENCE}"
```

必须输出：

```text
verify_m7_evidence: PASS
```

Verifier 会分别验证两种 evidence contract，再验证两份 evidence 的：

- config SHA256 一致；
- source_id 一致；
- connection_id 一致；
- marker SHA256 一致；
- logical target host SHA256 一致；
- BUILDINFO git commit 在双方均存在时一致；
- stdio Direct-path gap 为真；
- 两边三工具与 helper cleanup 都 PASS；
- systemd running binary 与 expected candidate hash 一致；
- evidence 中不存在约定禁止的敏感字段键。

## 13. `--self-test` 的含义

仓库/CI/package validator 可以执行：

```bash
python3 scripts/verify_m7_evidence.py --self-test
```

它只使用 synthetic in-memory evidence，用于证明 verifier 自身能接受合法记录并拒绝明显坏记录/敏感字段。

它的输出即使是 PASS，也**绝不代表** WSL、Windows helper、SSH/SFTP、systemd 或生产网络已经通过。

## 14. Evidence 保存

建议将本次两份 JSON 与以下非敏感元数据一起归档到内部测试记录：

```text
operator
timestamp
environment/candidate identifier
BUILDINFO git_commit
stdio evidence SHA256
HTTP evidence SHA256
verify_m7_evidence output
```

不要把 Secret、private key、passphrase、raw stderr、日志正文或明文 logical target host 放进 ticket/Issue/PR。

## 15. 失败处理

任意 Gate FAIL：

1. 保留原始去敏 FAIL evidence；
2. 按稳定 failure_code 分类；
3. 修复环境或代码；
4. candidate/config 若变化，重新执行 Gate A/B/C/D；
5. 不复用旧 PASS evidence 给新 candidate。

不要通过以下方式绕过失败：

```text
关闭 known_hosts
改成 root 运行
允许 stale cache 冒充远端成功
移除 helper cleanup 检查
使用 interactive shell 成功代替 systemd service
使用 --allow-direct-reachable 结果作为最终 WSL gap 证据
```

## 16. 最终 RC 条件

Real target 只在以下同时成立时完成：

```text
Gate A service-identity stdio        PASS
Gate B production healthcheck        PASS
Gate C systemd HTTP Proxy source     PASS
Gate D evidence pair verifier        PASS
```

此外，当前 candidate 的 Rust / Contracts / Direct SSH / 所有 M7 live gates / Performance / Release gates 仍必须真实执行并 PASS。

在这些条件全部满足前：

```text
PR #25 Ready     NO
merge            NO
tag/release      NO
production deploy NO
```
