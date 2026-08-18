# Log Query MCP v2 发布执行手册

> 用途：供后续 AI 或维护者从当前状态继续完成 Remote SSH/SFTP v2 的验证、合并与发布。
>
> 最近核对：2026-08-18
>
> 当前结论：实现和测试工具基本齐备，但候选版本尚未完成最终门禁、真实 WSL/目标机验收、合并和正式发布。

### 本轮接手核对（2026-08-18）

- 本地工作分支：`release/v2-rc`，从 `origin/feat/v2-m1-backend-config` 创建；验证候选仍为 `848ae2d04a4f78d1acbe7d4708dbd48e4b183e1b`。
- `origin/main` 仍为 `ce2abf108da6c7e8e709ddbaa3992066807f83c3`；候选远端引用同步后无变化。
- 工作区原有用户修改已保留：`README.md` 的文档索引变更和本手册文件；未覆盖或丢弃。
- PR #25：open、Draft、未合并，head/base 与上述候选和 `main` 一致。
- Issue #23：open，仍跟踪 GitHub Actions Billing/Spending Limit 与最终 CI/Performance/Release 门禁恢复。
- 候选 `848ae2d` 的最新 Actions runs（Contracts `31455716785`、Rust `31455716801`、Release `31455716807`、M7 ProxyCommand `31455716797`、Proxy Auth `31455716800`、Proxy Sync `31455716795`、ProxyCommand Failures `31455716794`、Mixed Query `31455716806`、Proxy Generation `31455716792`、Proxy Restart `31455716812`）均为 `completed/failure`，对应 job 的 `steps=null`；未执行 checkout/build/test/package，按外部 runner/Billing 阻塞处理，不能视为代码失败或 PASS。
- 本机 `gh` token 已失效且无法访问 GitHub API；上述 PR、Issue 和 Actions 事实已通过连接器重新核对。发布手册中的候选 SHA、PR/Issue 阻塞状态没有过期，无需改写为其他远端事实。

## 1. 最终目标

将 `feat/v2-m1-backend-config` 上的 Remote SSH/SFTP、缓存和 ProxyCommand 能力完成验证后合并到 `main`，创建新的版本 tag 和 GitHub Release，并在目标环境完成安装、查询、升级和回滚验收。

发布完成必须同时满足：

1. 候选提交上的本地 RC 检查通过。
2. GitHub Actions 的 Rust、Contracts、Direct SSH、ProxyCommand、性能和 Release 门禁全部真实执行并通过。
3. 真实 WSL/Windows ProxyCommand 验收形成可追溯的脱敏证据。
4. 目标 Linux 环境生产验收通过。
5. PR 完成评审并合并到 `main`。
6. 新版本 tag 与 `Cargo.toml` 版本一致。
7. GitHub Release 产物、校验和、安装、健康检查、升级和回滚均通过验证。

仅有代码、测试脚本或历史测试结果，不等于发布完成。

## 2. 当前基线

### 2.1 分支状态

```text
main                              ce2abf108da6c7e8e709ddbaa3992066807f83c3
origin/feat/v2-m1-backend-config  848ae2d04a4f78d1acbe7d4708dbd48e4b183e1b
Draft PR                          #25
外部阻塞 Issue                    #23
```

- `main` 当前为 v1 基线，只读取 MCP 所在 Linux 服务器上的本地日志。
- v1 的远程访问方式是客户端通过 MCP Streamable HTTP 调用远端服务，不是由服务通过 SSH 拉取其他服务器日志。
- v2 候选分支比 `main` 多 387 个提交。
- v2 候选实现了 Direct SSH、SFTP 只读访问、ProxyCommand、本地 generation cache、增量同步，以及 Local/Remote 混合查询。
- PR #25 当前为 Draft，尚未合并。
- GitHub Actions 最近因 Billing/Spending Limit 在 runner 启动前失败。此类 `FAILURE` 不是代码测试失败，也不能视为 PASS。

开始工作时必须重新获取远端状态，不得盲目依赖上述 SHA：

```bash
git fetch origin
git status --short --branch
git branch -a -vv
git log -1 --format='%H %ci %s' origin/main
git log -1 --format='%H %ci %s' origin/feat/v2-m1-backend-config
gh pr view 25 --json state,isDraft,headRefName,baseRefName,mergeable,statusCheckRollup,updatedAt,url
gh issue view 23 --json state,title,updatedAt,url
```

如果远端分支、PR 或计划文档已经更新，应以最新远端状态为准，并在继续前更新本节。

## 3. 已实现范围

以下项目在候选分支中已有代码或测试工具，后续工作的重点是验证、修复和交付，不应无依据地重新实现：

- v1/v2 配置路由和 v2 JSON Schema。
- `SourceBackend`、`LocalBackend` 和 `RemoteBackend`。
- `russh + russh-sftp` Direct SSH/SFTP 只读传输。
- 密码、私钥和加密私钥认证。
- strict `known_hosts` 主机身份校验。
- 管理员配置的 ProxyCommand raw byte-stream 传输。
- Remote 日志 generation cache、quota、GC 和 crash recovery。
- Full、Tail、FromNow 和增量同步。
- 日志追加、轮转、截断和替换处理。
- Remote cursor、`match_ref` 和 generation snapshot 一致性。
- Local、Direct Remote 和 Proxy Remote 混合查询。
- `list_log_sources`、`search_logs`、`get_log_context` 完整 MCP 链路。
- 安装、健康检查、升级、自动回滚和手工回滚脚本。
- Direct SSH、ProxyCommand、故障、安全、并发、性能和 WSL 验收工具。

## 4. 不可改变的安全边界

执行修复和发布时必须保持：

- AI-facing 工具只有 `list_log_sources`、`search_logs`、`get_log_context`。
- 不增加 `ssh_exec`、Shell、任意远程路径读取、上传、写入、删除、部署或重启功能。
- SSH 只作为内部传输层，远端访问只使用只读 SFTP 操作。
- host、port、username、credential、remote root 和 ProxyCommand 只能由管理员配置。
- AI 请求不能提供 SSH 主机、凭据、远程目录或 ProxyCommand 参数。
- ProxyCommand 使用固定 `program + args[]` 直接启动，不拼接 Shell command string。
- ProxyCommand 占位符仅允许完整 argv 元素 `{host}` 和 `{port}`。
- strict `known_hosts` 必须 fail-closed。
- Remote 日志必须先同步为稳定的本地 generation，再交给查询引擎。
- 不允许在同步失败时静默使用 stale cache。
- Tail/FromNow 覆盖不足必须返回 `CACHE_SCOPE_EXCEEDED`，不得产生假阴性。
- MCP 响应和验收证据不得泄漏 Secret、私钥、绝对路径、日志正文、内部 offset 或未脱敏主机信息。

## 5. 执行规则

### 5.1 工作分支

不要直接在 `main` 或远端候选分支上提交。工作区干净时，从最新候选创建独立分支：

```bash
git fetch origin
git switch -c release/v2-rc origin/feat/v2-m1-backend-config
```

如果同名本地分支已存在，先检查其上游、提交和工作区状态，不得强制重置或覆盖用户修改。

### 5.2 发布动作授权

以下动作属于外部发布操作，必须获得仓库所有者明确授权后才能执行：

- 将 PR 从 Draft 标记为 Ready。
- 合并 PR。
- 创建或推送 tag。
- 创建 GitHub Release。
- 部署或升级生产服务器。

诊断、编译、测试、构建 dry-run 包和生成本地证据不需要把 PR 标 Ready。

### 5.3 失败处理

每次门禁失败必须记录：

- 候选 commit SHA。
- 执行命令或 workflow run URL。
- 执行环境。
- 首个真实失败步骤。
- 错误分类：代码、测试夹具、基础设施、凭据、网络、Billing 或目标环境。
- 修复提交和复测结果。

不得通过跳过测试、放宽安全断言或删除门禁来制造绿灯。

## 6. 阶段 A：解除 CI 阻塞

1. 检查 Issue #23 和最新 workflow annotation。
2. 由有权限的管理员恢复 GitHub Actions Billing/Spending Limit。
3. 重新运行一个轻量 Contracts 或 Rust workflow，确认 runner 真正执行了 steps。
4. 确认不再出现以下 annotation：

```text
The job was not started because recent account payments have failed
or your spending limit needs to be increased.
```

5. Billing 恢复后保持 Issue #23 打开，直到全部候选门禁完成；随后记录结果并关闭。

完成标准：workflow 有真实 checkout/build/test 日志，而不是启动前失败。

如果当前 AI 无权处理 Billing，应报告该外部阻塞，同时继续执行不依赖 GitHub runner 的本地检查；不得宣称发布完成。

### 阶段 A 实际状态（2026-08-18）

- 状态：`BLOCKED-EXTERNAL`，已确认但无法由当前会话解除。
- 证据：PR [#25](https://github.com/liqiangcc/log-query-mcp/pull/25) 和 Issue [#23](https://github.com/liqiangcc/log-query-mcp/issues/23) 仍为 Draft/open；当前候选的上述 Actions jobs 全部 `steps=null`，没有 runner 执行日志。
- 当前会话无仓库 Billing/Spending Limit 管理权限；未重试或伪造 PASS，也未关闭 Issue #23。
- 阶段记录 commit：`848ae2d04a4f78d1acbe7d4708dbd48e4b183e1b`（候选未改动）。
- 后续动作：继续执行不依赖 GitHub runner 的阶段 B～H 本地检查；阶段 A 只有在管理员恢复 Billing 并重新运行轻量 Contracts/Rust workflow 后才能转为 `PASS`。

## 7. 阶段 B：确定并同步版本

候选分支起始使用 `0.1.0`，而仓库已经存在 `v0.1.0` tag，不能重复发布同一 tag。

默认建议新版本为 `0.2.0`，但在修改前必须确认：

```bash
git tag --list 'v*' --sort=-version:refname
gh release list
git show origin/feat/v2-m1-backend-config:Cargo.toml | sed -n '1,20p'
```

如果仓库所有者没有指定其他版本，则按下一兼容次版本 `0.2.0` 准备 RC。同步修改：

- `Cargo.toml` package version。
- `Cargo.lock` 中本包版本。
- README 下载和安装示例。
- 安装、运维、Release Readiness 和生产验收文档中的固定版本。
- 发布包名、BUILDINFO 和 tag/version 测试夹具。
- 任何仍把 Remote v2 正式包描述为 `v0.1.0` 的内容。

执行格式化和版本引用检查：

```bash
cargo check --locked --all-targets --all-features
rg -n '0\.1\.0|v0\.1\.0' README.md docs examples scripts .github Cargo.toml Cargo.lock
```

不是所有历史文档中的旧版本都必须替换。历史 v1 记录应保留；只修改会影响当前 v2 发布和操作指引的引用。

完成标准：新 tag 尚不存在，Cargo、发布脚本、包名、文档和 BUILDINFO 约定一致。

### 阶段 B 实际状态（2026-08-18）

- 状态：`PASS`（本地版本同步）；选择 `0.2.0`，远端不存在 `v0.2.0`。
- 已同步：`Cargo.toml`、`Cargo.lock` 本包条目、README/安装指南包引用、健康检查和 WSL/manifest/test 版本夹具。
- 保留第三方依赖 `sponge-cursor 0.1.0` 等非发布版本引用；没有替换历史 v1 记录。
- `bash scripts/check_release_tag.sh v0.2.0`：PASS；`git diff --check`：PASS。
- 全目标 `cargo check --locked --all-targets --all-features` 首次发现并修复 `map_ssh_transport` 未覆盖 5 个 ProxyCommand 错误；修复后：PASS。
- 阶段记录 commit：`d608e946ddd3f6456ce7d7507567c8be0641cde7`；该提交包含版本同步、ProxyCommand 错误映射修复和格式门禁修复。

## 8. 阶段 C：本地 RC 检查

### 8.1 环境依赖

至少需要：

- 与仓库 toolchain 要求兼容的 `cargo` 和 `rustc`。
- Python 3。
- 支持 `Draft202012Validator` 的 `jsonschema`。
- `sha256sum`、`tar` 和 Bash。
- `x86_64-unknown-linux-gnu` 构建目标及所需 linker。

先检查：

```bash
command -v cargo rustc python3 sha256sum tar
python3 - <<'PY'
from jsonschema import Draft202012Validator
print("jsonschema Draft 2020-12: OK")
PY
rustc --version
cargo --version
```

### 8.2 执行统一检查

```bash
bash scripts/rc_check.sh
```

该脚本应覆盖：

- v1/v2 contract 验证。
- ProxyCommand release contract。
- WSL 验收工具静态检查。
- evidence verifier 和 run manifest 自测。
- rustfmt。
- Clippy `-D warnings`。
- 全部非 live Rust tests。
- release binaries。
- MCP protocol health failure matrix。
- upgrade/rollback failure matrix。
- release package 构建和校验。

必须保存：

```text
commit SHA
rustc/cargo/python/jsonschema 版本
rc_check 完整日志
生成包名称
包 SHA256
BUILDINFO
```

完成标准：脚本明确输出 `rc_check: PASS`。

注意：`rc_check: PASS` 仍不能替代真实 Direct/Proxy SSH、性能、WSL 或生产验收。

### 阶段 C 实际状态（2026-08-18）

- 状态：`PASS`，候选 commit：`d608e946ddd3f6456ce7d7507567c8be0641cde7`。
- 完整日志：`/tmp/log-query-mcp-rc-check-d608e94.log`；末行明确为 `rc_check: PASS`。
- 环境：`rustc 1.97.1 (8bab26f4f 2026-07-14)`、`cargo 1.97.1`、Python 3.10.12、`jsonschema 4.23.0` Draft 2020-12；target `x86_64-unknown-linux-gnu`。
- 发布包：`/tmp/log-query-mcp-dist-d608e94/rc-check/log-query-mcp-v0.2.0-x86_64-unknown-linux-gnu.tar.gz`。
- 外部 archive SHA256：`cca89fd56692c7dd18dd05c690b95179e8b0591c7d3ea243bc63e199cee876e4`；包内 `SHA256SUMS` 外部文件 SHA256：`4b92f3b9a1181d6ed169afd03afcf280a9007eed96e2a15c20646eedda1008ee`。
- BUILDINFO：version `0.2.0`、commit `d608e946ddd3f6456ce7d7507567c8be0641cde7`、target `x86_64-unknown-linux-gnu`、rustc `1.97.1`。
- RC 中的 live SSH/Proxy、性能和真实 WSL 检查仍按脚本保持独立门禁，未被本地 RC 的 synthetic/self-test 代签。

## 9. 阶段 D：Direct SSH/SFTP 门禁

使用仓库 workflow 和测试夹具执行真实 OpenSSH/SFTP 测试。至少验证：

- 密码认证。
- 普通私钥认证。
- 加密私钥和 passphrase secret。
- 正确 host key 成功，错误 host key fail-closed。
- SFTP `lstat`、目录发现和 bounded range read。
- symlink escape 和非普通文件拒绝。
- 超时、断连、认证失败和取消。
- session semaphore 和故障后的 permit 释放。
- 300 次连续 range-read handle regression。
- Full、Tail、FromNow 和增量同步。
- append、rotation、truncate 和 replacement。
- Remote `search_logs`、分页和 `get_log_context`。
- Local + Remote 混合查询和单来源失败隔离。

相关入口以候选分支最新文件为准，重点包括：

```text
.github/workflows/ssh-research.yml
tests/ssh_transport_live.rs
tests/m4_sync_live.rs
tests/m5_remote_query_live.rs
tests/m6_remote_security_live.rs
tests/m6_multi_server_live.rs
tests/m6_server_restart_live.rs
```

完成标准：当前候选 SHA 上的 Direct SSH、安全、查询和恢复 live gates 全部 PASS。

## 10. 阶段 E：ProxyCommand 门禁

必须在当前候选 SHA 上执行：

1. ProxyCommand → OpenSSH → SFTP 成功链路。
2. 密码、私钥、加密私钥认证。
3. strict `known_hosts`。
4. Full、Tail、FromNow、append、rotation、truncate 同步。
5. program 不存在、不可执行、启动失败、EOF、stderr flood、超时、取消、认证失败和 helper crash。
6. 正常、失败和取消路径的 child kill/reap。
7. Direct/Proxy 共用 SSH semaphore 且彼此故障隔离。
8. Local + Direct + Proxy 混合查询。
9. 服务重启、stale-cache fail-closed 和恢复。
10. cursor、`match_ref`、generation pin 和 cache-only context 一致性。

重点 workflow/test：

```text
.github/workflows/m7-proxy-command.yml
.github/workflows/m7-proxy-auth.yml
.github/workflows/m7-proxy-sync.yml
.github/workflows/m7-proxy-command-failures.yml
.github/workflows/m7-mixed-query.yml
.github/workflows/m7-proxy-restart.yml
.github/workflows/m7-proxy-generation.yml
tests/m7_*.rs
```

完成标准：上述工作流真实执行并全部 PASS，没有孤儿 helper、敏感信息泄漏或无法解释的资源泄漏。

## 11. 阶段 F：性能和资源证据

运行 M7 Direct/Proxy paired performance gate，至少采集：

- Direct 与 Proxy 各 5 次 session setup latency。
- 100 MiB Full bootstrap。
- 1 GiB Full bootstrap。
- 10 GiB logical file 的 Tail 64 MiB bootstrap。
- unchanged continuity probe 传输量不超过设计上限。
- incremental append 只传输追加数据和有界探测。
- cache-local scan 不产生远程字节。
- ProxyCommand 300 次 range reads。
- 2 Direct + 2 Proxy 并发查询。
- CPU、最大 RSS、elapsed time、磁盘占用和网络字节。
- 每阶段结束后的 helper orphan 检查。

入口：

```text
.github/workflows/m7-proxy-performance.yml
tests/m7_proxy_performance_live.rs
docs/M7_PROXY_PERFORMANCE_GATE_V2.md
```

完成标准：workflow PASS，指标和环境信息作为 artifact 保存，并确认没有无法解释的性能、内存、句柄、磁盘或进程回归。

历史 M6 指标只能作为对照，不能替代当前 M7 候选执行。

## 12. 阶段 G：真实 WSL/Windows 验收

### 12.1 必须证明的场景

```text
WSL Direct target path unavailable
    +
Windows Host/VPN path available
    +
log-query-mcp service identity launches approved Windows helper
    +
ProxyCommand -> SSH -> strict known_hosts -> auth -> SFTP
    +
list_log_sources/search_logs/get_log_context PASS
    +
normal/failure/cancel helper cleanup PASS
```

### 12.2 执行入口

严格按照候选分支中的最新说明执行：

```text
docs/M7_REAL_TARGET_EXECUTION_RUNBOOK_V2.md
docs/M7_WSL_ACCEPTANCE_V2.md
docs/M7_WSL_SYSTEMD_HTTP_ACCEPTANCE_V2.md
scripts/m7_real_target_acceptance.sh
scripts/m7_real_target_manifest.py
scripts/m7_wsl_acceptance.sh
scripts/m7_wsl_acceptance.py
scripts/m7_wsl_http_acceptance.py
scripts/verify_m7_evidence.py
```

不得自行简化 service identity、binary SHA、systemd 或 helper cleanup 校验。

### 12.3 必须保存的证据

- 候选 commit、binary SHA256 和 BUILDINFO。
- WSL/Windows/目标 SSH 环境描述。
- Direct path 不可用和 Windows/VPN path 可用证据。
- 实际 systemd User、MainPID 和 `/proc/<pid>/exe` 对应候选 binary 的证据。
- stdio 和 Streamable HTTP MCP initialize/tools/list/三工具结果。
- helper 前后进程计数及失败/取消清理结果。
- strict host-key 和认证结果。
- evidence verifier PASS 结果。

证据必须脱敏，不保存 Secret、私钥、密码、完整日志正文、match_ref 或不必要的内部主机信息。

完成标准：真实 stdio 与 systemd HTTP 两套验收均 PASS，manifest 完整且 evidence verifier PASS。

## 13. 阶段 H：发布包和生命周期验收

生成正式 dry-run 包：

```bash
TARGET=x86_64-unknown-linux-gnu bash scripts/rc_check.sh
```

独立验证：

- archive 外部 SHA256。
- 包内 SHA256。
- BUILDINFO 的版本、commit、target 和构建时间。
- 两个 binary 可执行。
- v1/v2 example 和 v2 machine schema 存在。
- Direct + Proxy 示例均存在。
- install/uninstall/healthcheck/upgrade/rollback 脚本可执行。
- M6/M7 Release Readiness 和验收文档包含在包内。

在隔离环境执行：

- 全新安装。
- MCP protocol health check。
- 保留现有生产配置的正常升级。
- restart 或协议健康失败后的自动回滚。
- 显式手工回滚。
- 损坏包在修改系统前被拒绝。
- 回滚后服务和 MCP 协议恢复。

完成标准：package validator、健康检查和全部生命周期场景 PASS。

## 14. 阶段 I：目标 Linux 生产验收

在预生产或等价目标环境按最新 `docs/PRODUCTION_CHECKLIST.md` 执行：

- kernel、glibc、架构和磁盘容量。
- systemd 安装、启动、停止、重启和开机自启。
- 服务用户最小权限。
- Local 日志读取权限。
- Remote 专用 SSH 账号和只读权限。
- `known_hosts`、Secret、私钥及 cache 目录权限。
- 默认 loopback 监听和上层 ACL/TLS/认证边界。
- MCP initialize、tools/list、三个工具。
- Local、Direct、Proxy 和混合查询。
- 日志轮转、SSH 中断、权限变化、缓存损坏和恢复。
- 实际 AI 客户端调用。
- 升级和回滚演练。

完成标准：发布必需项有操作者、时间和证据；未执行项保持“待验收”，不得由 CI 代签。

## 15. 阶段 J：PR Ready、合并和发布

只有 A-I 全部完成后才能进入本阶段。

### 15.1 合并前

- 将工作分支同步到最新 `main`，解决冲突并重新执行受影响门禁。
- 更新 `REMOTE_SSH_CACHE_TODO_V2.md`、`PROXY_COMMAND_TODO_V2.md`、`RELEASE_READINESS_V2.md` 和生产验收状态。
- 在 PR 中附上当前候选 SHA、全部 workflow、性能 artifact、WSL evidence 和目标环境验收入口。
- 确认没有未解释的 required check failure。
- 获得明确授权后将 PR 标记为 Ready。
- 完成 review，并对每项修改重新执行相关门禁。

### 15.2 合并

获得明确授权后合并 PR。合并后记录：

```text
merge commit
合并时间
reviewer
最终候选与 merge commit 的关系
```

如果 merge commit 与已验证候选不同，至少重跑 Rust、Contracts、Release/package；影响 SSH、缓存或 transport 的改动必须重跑对应 live/performance gates。

### 15.3 Tag 和 Release

获得明确授权后，在已验证的 `main` 提交上创建与 Cargo 版本一致的新 tag，例如：

```bash
git switch main
git pull --ff-only origin main
bash scripts/check_release_tag.sh v0.2.0
git tag -a v0.2.0 -m 'Release v0.2.0'
git push origin v0.2.0
```

推送前再次确认 tag 不存在且指向正确提交。不得复用或移动已有发布 tag。

等待 Release workflow 完成后验证：

- GitHub Release 指向正确 tag/commit。
- archive 文件名与版本一致。
- `SHA256SUMS` 可校验下载的 archive。
- 解包后内部 checksum 和 BUILDINFO 正确。
- binary `--version` 或等价版本信息正确。
- 发布包可在干净目标机安装。

完成标准：正式 Release 可下载、可验证、可安装，发布后 MCP 和已批准 Remote 来源冒烟测试通过。

## 16. 发布完成判定

只有下表全部为 `PASS` 才能宣布 v2 发布完成：

| 项目 | 要求 |
|---|---|
| Version | 新版本唯一，Cargo/tag/package/BUILDINFO 一致 |
| Local RC | `scripts/rc_check.sh` PASS |
| Rust/Contracts | 当前候选 GitHub Actions PASS |
| Direct SSH | auth/security/sync/query/fault gates PASS |
| ProxyCommand | success/auth/sync/failure/mixed/restart/generation gates PASS |
| Performance | 当前候选 M7 metrics 和 artifact 完整，无未解释回归 |
| WSL | 真实 stdio + systemd HTTP evidence PASS |
| Package | checksum/BUILDINFO/package validator PASS |
| Lifecycle | install/health/upgrade/rollback PASS |
| Production | 目标 Linux 和实际 AI 客户端验收 PASS |
| PR | reviewed、merged，merge commit 可追溯 |
| Release | 新 tag 和 GitHub Release 已发布并复验 |

以下状态均不能称为“发布完成”：

- 代码已写但 live tests 未执行。
- workflow 因 Billing 在 runner 启动前失败。
- 只有历史 M6 证据，没有当前 M7 候选证据。
- 只有 WSL 静态检查，没有真实 Windows/VPN/SSH 链路。
- PR 仍为 Draft 或尚未合并。
- tag 已创建但 Release 产物未验证。
- CI 通过但目标环境安装、查询和回滚未验收。

## 17. 每次接手时的状态报告模板

后续 AI 每次完成一个阶段后，应按以下格式更新计划或向用户报告：

```markdown
## v2 发布状态 YYYY-MM-DD

- 当前分支：
- 当前 commit：
- PR 状态：
- Issue #23 状态：
- 本轮完成：
- 通过的门禁及 URL/证据：
- 失败的门禁及首个错误：
- 外部阻塞：
- 尚未完成：
- 下一步：
- 需要用户授权的动作：
```

最终交接时必须给出可验证的事实和证据位置，不得只写“应该已完成”或“看起来正常”。
