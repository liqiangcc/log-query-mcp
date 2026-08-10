# Log Query MCP 生产验收清单

本文用于首个生产发布和每次升级前后的验收。**没有在目标环境实际执行的项目必须保持为“待验收”**；历史 CI 或 harness 已实现不等同于当前 candidate 或目标服务器验收。

## 1. 自动验证项

这些项目由 CI 或本地可重复验证覆盖；当前 candidate 的 GitHub Actions runner 仍受 Billing/Spending Limit 外部阻塞，因此所有 M7 live/performance 项必须区分“harness 已实现”和“PASS evidence”。

| 项目 | 状态 | 证据 |
|---|---|---|
| Rust 格式检查 | 历史通过；当前 candidate 待重跑 | `cargo fmt --all -- --check` |
| Clippy 严格检查 | 历史通过；当前 candidate 待重跑 | `cargo clippy --locked --all-targets --all-features -- -D warnings` |
| 全量测试 | 历史通过；当前 candidate 待重跑 | `cargo test --locked --all-targets --all-features` |
| v1/v2 contract/schema | harness 已实现；当前 candidate 待重跑 | `python3 scripts/validate_contracts.py` |
| ProxyCommand release contract | 已接入本地 RC；当前 candidate 待实际执行 | `scripts/rc_check.sh` |
| release binaries | 历史通过；当前 candidate 待重跑 | `cargo build --release --locked --bins` |
| stdio / Streamable HTTP smoke | 历史通过；当前 candidate 待重跑 | `tests/mcp_transport_smoke.rs` |
| Direct Password/private-key/host-key/timeout SFTP live matrix | 历史通过；M7 修改后待重跑 | `SSH Transport` workflow |
| M7 ProxyCommand success + strict host-key | harness 已实现 / execution blocked | `M7 ProxyCommand` |
| M7 private/encrypted-key auth | harness 已实现 / execution blocked | `M7 Proxy Auth` |
| M7 full/tail/from_now/incremental/rotation/truncate | harness 已实现 / execution blocked | `M7 Proxy Sync` |
| M7 startup/auth/timeout/cancel/crash failure matrix | harness 已实现 / execution blocked | `M7 ProxyCommand Failures` |
| Local + Direct + Proxy mixed query / failed-Proxy isolation | harness 已实现 / execution blocked | `M7 Mixed Query` |
| Proxy restart / stale-cache fail-closed / recovery | harness 已实现 / execution blocked | `M7 Proxy Restart` |
| Proxy cursor/match_ref/generation pin | harness 已实现 / execution blocked | `M7 Proxy Generation` |
| Direct/Proxy paired performance + 300 reads + concurrency | harness 已实现 / execution blocked | `M7 Proxy Performance` |
| M6 100 MiB / 1 GiB / 10 GiB-tail historical benchmark | 历史 evidence 已有 | `M6_PERFORMANCE_BASELINE_V2.md` |
| protocol health failure matrix | 本地已验证；当前 candidate 待重跑 | `tests/healthcheck_test.sh` |
| release package assembly | 脚本已支持 M7 artifacts；当前 candidate 待重跑 | `scripts/package_release.sh` |
| package completeness + 内/外 SHA256 + Proxy release contract | validator 已实现；当前 candidate 待重跑 | `scripts/validate_release_package.sh` |
| upgrade/rollback 隔离演练 | 本地历史验证；当前 candidate 待重跑 | `tests/upgrade_rollback_test.sh` |
| 全部非 live-SSH 本地 RC Gate | 一键入口已实现 | `scripts/rc_check.sh` |
| tag 与 Cargo version 一致性 | Gate 已存在 | `scripts/check_release_tag.sh` |

> Issue #23 跟踪当前 GitHub Actions Billing/Spending Limit blocker。任何 `steps=null` workflow failure 都不能视为 code failure，也不能视为 PASS。

## 2. Release Candidate Gate

候选 commit 必须满足：

- [ ] `bash scripts/rc_check.sh` 在候选 commit 对应源码成功。
- [ ] Rust workflow 在**候选 commit**成功。
- [ ] Contracts workflow 在**候选 commit**成功。
- [ ] Direct SSH Transport workflow 在**候选 commit**成功。
- [ ] `M7 ProxyCommand` success/host-key gate 成功。
- [ ] `M7 Proxy Auth` 成功。
- [ ] `M7 Proxy Sync` 成功。
- [ ] `M7 ProxyCommand Failures` 成功。
- [ ] `M7 Mixed Query` 成功。
- [ ] `M7 Proxy Restart` 成功。
- [ ] `M7 Proxy Generation` 成功。
- [ ] `M7 Proxy Performance` 成功，并产出当前 candidate Direct/Proxy paired metrics。
- [ ] Release package job 在**候选 commit**成功。
- [ ] Release artifact 通过 `validate_release_package.sh`。
- [ ] `healthcheck_test.sh` 成功。
- [ ] `upgrade_rollback_test.sh` 成功。
- [ ] 真实 WSL → Windows Host helper → Remote SSH acceptance 可追溯并成功。
- [ ] 没有未解释的 workflow failure/cancelled/skipped critical step。

只有 Billing/runner 等外部阻塞解除、全部 Gate 执行并通过、真实 WSL acceptance 完成后，才可标记 RC Ready。

## 3. 发布包验收

| 项目 | 状态 | 记录 |
|---|---|---|
| GitHub Release tag 为 `v{Cargo.toml package.version}` | 待正式发布验收 |  |
| 下载 tar.gz 和外层 `SHA256SUMS` | 待正式发布验收 |  |
| 外层 `sha256sum -c SHA256SUMS` 通过 | 待正式发布验收 |  |
| `validate_release_package.sh` 通过 | 待正式发布验收 |  |
| 包内 `SHA256SUMS` 全部通过 | 待正式发布验收 |  |
| `BUILDINFO` 包含 version/target/commit/ref/build time/rustc | 待正式发布验收 |  |
| 包含 install/uninstall/healthcheck/upgrade/rollback | 待正式发布验收 |  |
| 包含 v1/v2 example | 待正式发布验收 |  |
| v2 example 同时包含 Direct SSH 与 ProxyCommand | 待正式发布验收 |  |
| 包含 `schemas/log-query-mcp-config-v2.schema.json` | 待正式发布验收 |  |
| v2 machine schema 包含 `ProxyCommandConfig` | 待正式发布验收 |  |
| 包含 INSTALL/OPERATIONS/PRODUCTION_CHECKLIST | 待正式发布验收 |  |
| 包含 CONFIG_SCHEMA_V2 / PROXY_COMMAND_TRANSPORT_V2 | 待正式发布验收 |  |
| 包含 M7 implementation/live/auth/sync/failure/restart/generation/performance docs | 待正式发布验收 |  |
| 包含 M6_PERFORMANCE/M6_FINAL/RELEASE_READINESS | 待正式发布验收 |  |

## 4. 目标服务器安装验收

| 项目 | 状态 | 记录 |
|---|---|---|
| Linux kernel `>= 5.6` | 待验收 |  |
| `x86_64` glibc 环境 | 待验收 |  |
| systemd 可用 | 待验收 |  |
| curl 可用于标准 protocol health check | 待验收 |  |
| `scripts/install.sh` 以 root 成功执行 | 待验收 |  |
| `log-query-mcp` 用户/组最小权限 | 待验收 |  |
| binaries 位于 `/opt/log-query-mcp/bin` | 待验收 |  |
| config 位于 `/etc/log-query-mcp/config.json` | 待验收 |  |
| cache root 位于受控 `/var/lib/log-query-mcp/cache` | 待验收 |  |
| systemd unit 正确安装 | 待验收 |  |
| config 仅 root 可写、服务组可读 | 待验收 |  |
| 如使用 ProxyCommand，helper 程序来源/路径/版本已审批 | 待验收或不适用 |  |
| 如使用 ProxyCommand，服务身份可执行 helper | 待验收或不适用 |  |

## 5. Local Source 配置/权限验收

| 项目 | 状态 | 记录 |
|---|---|---|
| 所有 Local `source_id` 已审批 | 待验收 |  |
| root 为必要的绝对目录，不是过宽 `/`/`/var` | 待验收 |  |
| files/directories 与 rotation 策略一致 | 待验收 |  |
| 服务用户只读批准日志 | 待验收 |  |
| 非白名单日志不能通过 MCP 访问 | 待验收 |  |
| openat2 安全前提（kernel/mount）满足 | 待验收 |  |

## 6. Remote SSH Source 验收

| 项目 | 状态 | 记录 |
|---|---|---|
| 每个 connection/source 已审批 | 待验收 |  |
| 使用专用 read-only 账号，无 sudo | 待验收 |  |
| 条件允许时启用 SFTP-only/chroot | 待验收或不适用 |  |
| Password 只通过 `secret_ref` 提供 | 待验收或不适用 |  |
| Private key 文件权限最小化 | 待验收或不适用 |  |
| encrypted key passphrase 通过 Secret reference | 待验收或不适用 |  |
| known_hosts 已通过独立可信渠道核对 fingerprint | 待验收 |  |
| host-key mismatch 实际 fail-closed | 待验收 |  |
| AI/API 无法提交 host/username/secret_ref/remote root | 待验收 |  |
| 服务端没有为 MCP 开放 Remote Exec/Shell | 待验收 |  |
| Remote 账号不能写/删目标日志 | 待验收 |  |

### 6.1 ProxyCommand 专项验收

| 项目 | 状态 | 记录 |
|---|---|---|
| 无 `proxy` 的 connection 继续使用 Direct TCP | 待验收 |  |
| ProxyCommand 仅管理员静态配置 | 待验收 |  |
| 使用 direct `program + argv[]`，没有 Shell command string | 待验收 |  |
| placeholder 仅完整 `{host}` / `{port}` argv 元素 | 待验收 |  |
| argv/env/stdin 不含 SSH password/passphrase/remote path | 待验收 |  |
| helper stdout 仅承载 TCP/SSH 字节流，无 banner/debug | 待验收 |  |
| strict known_hosts 仍验证逻辑 target `host:port` | 待验收 |  |
| wrong host key 通过 Proxy path 仍 fail-closed | 待验收 |  |
| wrong credential 仍分类为 auth failure | 待验收 |  |
| Proxy early exit/timeout/crash 不泄露 raw stderr/argv | 待验收 |  |
| cancellation/timeout/normal close 后无 orphan helper | 待验收 |  |
| Proxy child 受同一 `max_concurrent_ssh_connections` 约束 | 待验收 |  |
| 一个 failed Proxy source 不污染 Local/Direct/其他 Proxy source | 待验收 |  |
| Proxy outage 时 `allow_stale_on_error=false` 不返回 stale-success | 待验收 |  |
| Proxy 恢复后可重新同步并查询 | 待验收 |  |

## 7. WSL → Windows Host 验收

该项只在目标部署确实依赖 WSL/宿主机网络时适用，但 M7 RC 必须有至少一份可追溯真实 acceptance evidence。

| 项目 | 状态 | 记录 |
|---|---|---|
| WSL 内 Direct SSH target path 已确认不可用 | 待验收 |  |
| Windows Host/VPN 到 target `host:port` 可用 | 待验收 |  |
| Windows helper executable 来源和版本已审批 | 待验收 |  |
| WSL Windows executable interop 对 service identity 可用 | 待验收 |  |
| systemd hardening 未被整体关闭 | 待验收 |  |
| ProxyCommand 可完成 SSH handshake + strict host key | 待验收 |  |
| password 或 key auth 成功 | 待验收 |  |
| SFTP read-only 成功 | 待验收 |  |
| `list_log_sources` PASS | 待验收 |  |
| `search_logs` PASS | 待验收 |  |
| `get_log_context` PASS | 待验收 |  |
| helper 正常/失败/取消路径无残留进程 | 待验收 |  |
| MCP 输出不泄露 Windows helper path/argv/Secret | 待验收 |  |

## 8. Cache 与容量验收

| 项目 | 状态 | 记录 |
|---|---|---|
| cache filesystem 容量满足 bootstrap + retention + generations | 待验收 |  |
| cache directory/file 权限符合 0700/0600 | 待验收 |  |
| cache/manifest 不含 Secret | 待验收 |  |
| global/per-source quota 与日志规模匹配 | 待验收 |  |
| Tail/FromNow 的覆盖边界符合业务查询需求 | 待验收 |  |
| `CACHE_SCOPE_EXCEEDED` 被客户端视为“不完整覆盖”而非“无结果” | 待验收 |  |
| 不使用人工删除 current/pinned generation 作为常规清理方式 | 待验收 |  |

## 9. 服务启动与 MCP 验收

| 项目 | 状态 | 记录 |
|---|---|---|
| `systemctl enable --now` 成功 | 待验收 |  |
| systemd active / journal 无 panic | 待验收 |  |
| 默认只监听 `127.0.0.1:8000` | 待验收 |  |
| `scripts/healthcheck.sh` 成功 | 待验收 |  |
| HTTP `/mcp` initialize 成功 | 待验收 |  |
| MCP Inspector 显示三个工具 | 待验收 |  |
| `list_log_sources` 只返回批准来源 | 待验收 |  |
| list/result/error 不泄露 host、username、secret、remote/cache absolute path、Proxy argv/stderr | 待验收 |  |
| Local `search_logs` 返回已知样例 | 待验收 |  |
| Direct Remote `search_logs` 返回已知样例 | 待验收 |  |
| Proxy Remote `search_logs` 返回已知样例 | 待验收 |  |
| Local + Direct + Proxy mixed query 正确 | 待验收 |  |
| `get_log_context(match_ref)` 返回有限上下文 | 待验收 |  |

## 10. Remote 故障/恢复验收

| 项目 | 状态 | 记录 |
|---|---|---|
| SSH server 不可用时 fail-closed | 待验收 |  |
| Proxy helper 不可用/early exit/timeout 时 fail-closed | 待验收或不适用 |  |
| auth failure 不泄露 Secret | 待验收 |  |
| host key 变化时拒绝连接 | 待验收 |  |
| server restart 后查询恢复 | 待验收 |  |
| append 只同步新增 range | 待验收 |  |
| rotation/truncate/replacement 创建正确 generation | 待验收 |  |
| 同步失败不破坏最后有效 cache | 待验收 |  |
| `allow_stale_on_error=false` 不把旧 cache 当成功结果 | 待验收 |  |
| 一台 Remote/Proxy Server 失败不污染另一台 cache | 待验收 |  |
| existing cursor/match_ref 继续绑定 pinned generation | 待验收 |  |

## 11. 性能/资源验收

| 项目 | 状态 | 记录 |
|---|---|---|
| M7 Direct 5-session setup metric 已记录 | 待当前 candidate evidence |  |
| M7 Proxy 5-session setup metric 已记录 | 待当前 candidate evidence |  |
| 100 MiB Direct + Proxy paired full profile PASS | 待当前 candidate evidence |  |
| 1 GiB Direct + Proxy paired full profile PASS | 待当前 candidate evidence |  |
| 10 GiB logical tail Direct + Proxy paired profile PASS | 待当前 candidate evidence |  |
| unchanged remote read `<= 64 KiB` | 待当前 candidate evidence |  |
| append remote read `<= payload + 2 × 64 KiB` | 待当前 candidate evidence |  |
| cache local scan remote bytes = 0 | 待当前 candidate evidence |  |
| Proxy 300 bounded range reads PASS | 待当前 candidate evidence |  |
| 2 Direct + 2 Proxy concurrency PASS | 待当前 candidate evidence |  |
| benchmark 后无 orphan Proxy helper | 待当前 candidate evidence |  |
| 无未解释 memory/disk/network regression | 待当前 candidate evidence |  |

M6 elapsed 数字仅作历史对照，不是 M7 PASS threshold，也不是产品 SLA。

## 12. 升级与回滚验收

CI/本地测试证明脚本逻辑，但目标服务器仍需执行一次受控演练。

| 项目 | 状态 | 记录 |
|---|---|---|
| 升级前外层 SHA256 验证 | 待验收 |  |
| `upgrade.sh` 创建 backup path | 待验收 |  |
| 正常升级保持 production config | 待验收 |  |
| 新 binaries/unit 原子替换后 `healthcheck.sh` 成功 | 待验收 |  |
| systemd active 但 MCP initialize 错误时升级被判失败 | 待验收 |  |
| 显式 `rollback.sh` 成功恢复旧版本 | 待验收 |  |
| restart/protocol-health failure 的自动 rollback 已演练 | 待验收 |  |
| rollback 后 `healthcheck.sh` + MCP smoke query 正常 | 待验收 |  |
| Proxy config/helper 在升级和回滚后仍按预期工作 | 待验收或不适用 |  |
| backup retention/清理策略已确定 | 待验收 |  |

## 13. 实际 AI 客户端验收

| 项目 | 状态 | 记录 |
|---|---|---|
| 客户端使用 Streamable HTTP | 待验收 |  |
| 可列出工具和搜索真实日志 | 待验收 |  |
| 可基于 match_ref 读取上下文 | 待验收 |  |
| 无法请求任意服务器路径 | 待验收 |  |
| 无法提交/修改 ProxyCommand | 待验收 |  |
| 无法触发 Remote Exec/写操作 | 待验收 |  |
| incomplete cache/error 能被正确理解 | 待验收 |  |

## 14. 非 loopback 暴露验收

服务不内置认证/TLS。只有确有需要时才允许非 loopback 暴露。

| 项目 | 状态 | 记录 |
|---|---|---|
| 已记录暴露原因和审批 | 待验收或不适用 |  |
| 使用反向代理/网关/内网 ACL | 待验收或不适用 |  |
| TLS 由可信上游终止 | 待验收或不适用 |  |
| 只允许批准 AI 客户端访问 | 待验收或不适用 |  |
| Inspector/调试入口未暴露不可信网络 | 待验收或不适用 |  |

## 15. 发布签署

```text
版本:
release tag:
candidate commit:
Issue #23 final-gate blocker status:
local rc_check result:
Release workflow run:
Rust run:
Contracts run:
Direct SSH run:
M7 Proxy success run:
M7 Proxy Auth run:
M7 Proxy Sync run:
M7 Proxy Failure run:
M7 Mixed Query run:
M7 Proxy Restart run:
M7 Proxy Generation run:
M7 Proxy Performance run/artifact:
WSL acceptance evidence:
发布包 SHA256:
目标服务器:
配置/审批单:
Proxy helper program/version/path（如适用）:
known_hosts fingerprint 审核记录:
自动验证结论:
人工验收结论:
遗留风险:
backup path:
回滚方案:
验收人:
验收时间:
```
