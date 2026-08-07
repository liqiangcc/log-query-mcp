# Log Query MCP 生产验收清单

本文用于首个生产发布和每次升级前后的验收。**没有在目标环境实际执行的项目必须保持为“待验收”**；CI 通过不等同于目标服务器验收。

## 1. 自动验证项

这些项目由 CI 或本地可重复验证覆盖。

| 项目 | 状态 | 证据 |
|---|---|---|
| Rust 格式检查 | 已自动验证过 | `cargo fmt --all -- --check` |
| Clippy 严格检查 | 已自动验证过 | `cargo clippy --locked --all-targets --all-features -- -D warnings` |
| 全量测试 | 已自动验证过 | `cargo test --locked --all-targets --all-features` |
| v1/v2 contract/schema | 已自动验证过 | `python3 scripts/validate_contracts.py` |
| release binaries | 已自动验证过 | `cargo build --release --locked --bins` |
| stdio / Streamable HTTP smoke | 已自动验证过 | `tests/mcp_transport_smoke.rs` |
| Password/private-key/host-key/timeout SFTP live matrix | 已自动验证过 | `SSH Transport` workflow |
| Remote query / cache / rotation / restart / fail-closed | 已自动验证过 | M4/M5/M6 live tests |
| 两台独立 SSH Server + Local mixed query | 已自动验证过 | `m6_multi_server_live.rs` |
| 300 次连续 bounded range read | 已自动验证过 | `ssh_transport_live.rs` |
| 100 MiB / 1 GiB / 10 GiB-tail benchmark | 已自动验证过 | `M6_PERFORMANCE_BASELINE_V2.md` |
| single/dual-server concurrency harness | 已实现并接入 live Gate | `m6_concurrency_performance_live.rs` |
| protocol health failure matrix | 本地已验证；CI Gate 已加入 | `tests/healthcheck_test.sh` |
| release package assembly | 已自动验证过 | `scripts/package_release.sh` |
| package completeness + 内/外 SHA256 | 本地 validator 已验证；CI Gate 已加入 | `scripts/validate_release_package.sh` |
| upgrade/rollback 隔离演练 | 本地已验证；CI Gate 已加入 | `tests/upgrade_rollback_test.sh` |
| 全部非 live-SSH 本地 RC Gate | 一键入口已实现 | `scripts/rc_check.sh` |
| tag 与 Cargo version 一致性 | Gate 已存在 | `scripts/check_release_tag.sh` |

> 当前 feature 分支最新 CI 重新执行受到 GitHub Actions Billing/Spending Limit 外部阻塞，跟踪于 Issue #23。正式 RC/Release 前必须在 Billing 恢复后让 candidate commit 的全部 Gate 再次变绿；历史成功 Gate 和本地脚本验证不能替代这一步。

## 2. Release Candidate Gate

候选 commit 必须满足：

- [ ] `bash scripts/rc_check.sh` 在候选 commit 对应源码成功。
- [ ] Rust workflow 在**候选 commit**成功。
- [ ] Contracts workflow 在**候选 commit**成功。
- [ ] SSH Transport workflow 在**候选 commit**成功，包含 single/dual-server concurrency benchmark。
- [ ] single/dual-server concurrency elapsed metrics 已记录到 `M6_PERFORMANCE_BASELINE_V2.md`。
- [ ] M6 Performance workflow 在**候选 commit 或未改变相关代码的可追溯 commit**成功。
- [ ] Release package job 在**候选 commit**成功。
- [ ] Release artifact 通过 `validate_release_package.sh`。
- [ ] `healthcheck_test.sh` 成功。
- [ ] `upgrade_rollback_test.sh` 成功。
- [ ] 没有未解释的 workflow failure/cancelled/skipped critical step。

只有 Billing/runner 等外部阻塞解除并完成上述 Gate，才可标记 RC Ready。

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
| 包含 INSTALL/OPERATIONS/PRODUCTION_CHECKLIST/M6_PERFORMANCE/M6_FINAL/RELEASE_READINESS | 待正式发布验收 |  |

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

## 7. Cache 与容量验收

| 项目 | 状态 | 记录 |
|---|---|---|
| cache filesystem 容量满足 bootstrap + retention + generations | 待验收 |  |
| cache directory/file 权限符合 0700/0600 | 待验收 |  |
| cache/manifest 不含 Secret | 待验收 |  |
| global/per-source quota 与日志规模匹配 | 待验收 |  |
| Tail/FromNow 的覆盖边界符合业务查询需求 | 待验收 |  |
| `CACHE_SCOPE_EXCEEDED` 被客户端视为“不完整覆盖”而非“无结果” | 待验收 |  |
| 不使用人工删除 current/pinned generation 作为常规清理方式 | 待验收 |  |

## 8. 服务启动与 MCP 验收

| 项目 | 状态 | 记录 |
|---|---|---|
| `systemctl enable --now` 成功 | 待验收 |  |
| systemd active / journal 无 panic | 待验收 |  |
| 默认只监听 `127.0.0.1:8000` | 待验收 |  |
| `scripts/healthcheck.sh` 成功 | 待验收 |  |
| HTTP `/mcp` initialize 成功 | 待验收 |  |
| MCP Inspector 显示三个工具 | 待验收 |  |
| `list_log_sources` 只返回批准来源 | 待验收 |  |
| list/result/error 不泄露 host、username、secret、remote/cache absolute path | 待验收 |  |
| Local `search_logs` 返回已知样例 | 待验收 |  |
| Remote `search_logs` 返回已知样例 | 待验收 |  |
| Local + Remote mixed query 正确 | 待验收 |  |
| `get_log_context(match_ref)` 返回有限上下文 | 待验收 |  |

## 9. Remote 故障/恢复验收

| 项目 | 状态 | 记录 |
|---|---|---|
| SSH server 不可用时 fail-closed | 待验收 |  |
| auth failure 不泄露 Secret | 待验收 |  |
| host key 变化时拒绝连接 | 待验收 |  |
| server restart 后查询恢复 | 待验收 |  |
| append 只同步新增 range | 待验收 |  |
| rotation/truncate/replacement 创建正确 generation | 待验收 |  |
| 同步失败不破坏最后有效 cache | 待验收 |  |
| 一台 Remote Server 失败不污染另一台 cache | 待验收 |  |

## 10. 升级与回滚验收

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
| backup retention/清理策略已确定 | 待验收 |  |

## 11. 实际 AI 客户端验收

| 项目 | 状态 | 记录 |
|---|---|---|
| 客户端使用 Streamable HTTP | 待验收 |  |
| 可列出工具和搜索真实日志 | 待验收 |  |
| 可基于 match_ref 读取上下文 | 待验收 |  |
| 无法请求任意服务器路径 | 待验收 |  |
| 无法触发 Remote Exec/写操作 | 待验收 |  |
| incomplete cache/error 能被正确理解 | 待验收 |  |

## 12. 非 loopback 暴露验收

服务不内置认证/TLS。只有确有需要时才允许非 loopback 暴露。

| 项目 | 状态 | 记录 |
|---|---|---|
| 已记录暴露原因和审批 | 待验收或不适用 |  |
| 使用反向代理/网关/内网 ACL | 待验收或不适用 |  |
| TLS 由可信上游终止 | 待验收或不适用 |  |
| 只允许批准 AI 客户端访问 | 待验收或不适用 |  |
| Inspector/调试入口未暴露不可信网络 | 待验收或不适用 |  |

## 13. 发布签署

```text
版本:
release tag:
candidate commit:
Issue #23 final-gate blocker status:
local rc_check result:
Release workflow run:
Rust run:
Contracts run:
SSH live run:
Performance run:
发布包 SHA256:
目标服务器:
配置/审批单:
known_hosts fingerprint 审核记录:
自动验证结论:
人工验收结论:
遗留风险:
backup path:
回滚方案:
验收人:
验收时间:
```
