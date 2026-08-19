# Log Query MCP v2 发布执行手册

> 用途：供后续 AI 或维护者从当前状态继续完成 Remote SSH/SFTP v2 的验证、合并与发布。
>
> 最近核对：2026-08-19
>
> 当前结论：v2.0 发布范围已正式限定为 Local + Direct SSH/SFTP；本地 A～D、H 门禁和 Direct/TUN 真实候选链路已通过。ProxyCommand 实现与测试保留为 post-v2 延后项；目标 Linux 生产验收、合并和正式发布仍未完成。

### 本轮接手核对（2026-08-19）

- 本地/远端独立工作分支：`release/v2-rc`，从 `origin/feat/v2-m1-backend-config` 创建；当前 head 为 `eab4ab29afe81c890f2398f2aef24ed85a454bb8`（短 SHA：`eab4ab2`）。冻结候选仍为 `848ae2d04a4f78d1acbe7d4708dbd48e4b183e1b`。
- `origin/main` 仍为 `ce2abf108da6c7e8e709ddbaa3992066807f83c3`；候选远端引用同步后无变化。
- 工作区原有用户修改已保留：`README.md` 的文档索引变更和本手册文件；未覆盖或丢弃。
- PR #25：open、Draft、未合并，head/base 与上述候选和 `main` 一致。
- 独立 scope PR #30：open、Draft、未合并，head=`release/v2-rc`/`eab4ab2`，base=`feat/v2-m1-backend-config`/`848ae2d`；仅用于在不改动候选分支的前提下运行本轮 v2.0 scope 变更的远端检查。
- Issue #23：open，仍跟踪 GitHub Actions Billing/Spending Limit 与最终 CI/Performance/Release 门禁恢复。
- Issue #26：已由本轮 scope decision 关闭，state reason=`not_planned`；原因是 ProxyCommand 已移出 v2.0 范围，不代表功能 PASS。Issue #27：open，`main` 发布保护/程序化 fallback 尚未完成。
- 本轮 scope decision 已落地在本地独立分支 commit `b680f73`：v2.0 package/example/required gates 改为 Direct-only；ProxyCommand 实现、测试和历史设计文档保留为 post-v2。
- v2.0 RC 入口移除 post-v2 WSL evidence/manifest synthetic self-test 的清理提交为 `a02bc63`；RC 仍保留 contracts、Rust、package/lifecycle 等 v2.0 检查。
- 候选 `848ae2d` 的最新 Actions runs（Contracts `31455716785`、Rust `31455716801`、Release `31455716807`、M7 ProxyCommand `31455716797`、Proxy Auth `31455716800`、Proxy Sync `31455716795`、ProxyCommand Failures `31455716794`、Mixed Query `31455716806`、Proxy Generation `31455716792`、Proxy Restart `31455716812`）均为 `completed/failure`，对应 job 的 `steps=null`；未执行 checkout/build/test/package，按外部 runner/Billing 阻塞处理，不能视为代码失败或 PASS。
- 独立 PR #30 的 head `7a39458eb44fba6e2c54518cb7c85346b365550c` 已触发新一轮 Actions；Contracts `32212531957`、Rust `32212531923`、Release `32212531905` 及 7 个历史 M7 workflow 均为 `completed/failure`，job `steps=[]`。Contracts check annotation 明确为 `recent account payments have failed or your spending limit needs to be increased`；因此仍是 Billing/runner 外部阻塞，不是代码失败，也不能视为 PASS。
- 本机 `gh` CLI 当前已登录 `liqiangcc`，Git 操作使用 HTTPS；PR、Issue 和 Actions 事实已重新核对。候选 SHA、PR/Issue 阻塞状态以冻结候选为准；独立 scope PR #30 已记录为新的远端验证入口。

## 1. 最终目标

将 `feat/v2-m1-backend-config` 上的 Remote SSH/SFTP（v2.0 为 Direct TCP）、缓存能力完成验证后合并到 `main`，创建新的版本 tag 和 GitHub Release，并在目标环境完成安装、查询、升级和回滚验收。ProxyCommand 实现不删除，正式延后到 post-v2。

发布完成必须同时满足：

1. 候选提交上的本地 RC 检查通过。
2. GitHub Actions 的 Rust、Contracts、Direct SSH、Direct 性能和 Release 门禁全部真实执行并通过。
3. 按目标部署需要完成 Direct/WSL 验收并形成可追溯的脱敏证据；不要求 Windows ProxyCommand helper。
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
独立 scope Draft PR               #30 (`release/v2-rc` -> `feat/v2-m1-backend-config`)
外部阻塞 Issue                    #23
```

- `main` 当前为 v1 基线，只读取 MCP 所在 Linux 服务器上的本地日志。
- v1 的远程访问方式是客户端通过 MCP Streamable HTTP 调用远端服务，不是由服务通过 SSH 拉取其他服务器日志。
- v2 候选分支比 `main` 多 387 个提交。
- v2 候选实现了 Direct SSH、SFTP 只读访问、本地 generation cache、增量同步，以及 Local/Remote 混合查询；ProxyCommand 实现保留但已移出 v2.0 发布范围。
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
- 管理员配置的 ProxyCommand raw byte-stream 传输（post-v2 延后实现，不是 v2.0 发布能力）。
- Remote 日志 generation cache、quota、GC 和 crash recovery。
- Full、Tail、FromNow 和增量同步。
- 日志追加、轮转、截断和替换处理。
- Remote cursor、`match_ref` 和 generation snapshot 一致性。
- Local、Direct Remote 混合查询；Proxy Remote 混合查询保留为 post-v2 回归范围。
- `list_log_sources`、`search_logs`、`get_log_context` 完整 MCP 链路。
- 安装、健康检查、升级、自动回滚和手工回滚脚本。
- Direct SSH、故障、安全、并发和性能验收工具；ProxyCommand/WSL helper 验收工具保留为 post-v2。

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

### 阶段 A 实际状态（2026-08-19）

- 状态：`BLOCKED-EXTERNAL`，已确认但无法由当前会话解除。
- 证据：PR [#25](https://github.com/liqiangcc/log-query-mcp/pull/25) 仍为 Draft，Issue [#23](https://github.com/liqiangcc/log-query-mcp/issues/23) 仍 open；独立 PR [#30](https://github.com/liqiangcc/log-query-mcp/pull/30) 也是 Draft。PR #30 head `7a39458` 的 Contracts `32212531957`、Rust `32212531923`、Release `32212531905` 等 Actions jobs 全部在启动前失败，`steps=[]`，check annotation 指向账户付款/Spending Limit。
- 当前会话无仓库 Billing/Spending Limit 管理权限；未重试或伪造 PASS，也未关闭 Issue #23。
- 阶段记录 commit：`7a39458eb44fba6e2c54518cb7c85346b365550c`（独立 scope 分支；候选 `848ae2d` 未改动）。
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
- Direct-only v2 release contract。
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

注意：`rc_check: PASS` 仍不能替代真实 Direct SSH、Direct 性能、适用的 WSL 或生产验收；ProxyCommand 不属于 v2.0 required gate。

### 阶段 C 实际状态（2026-08-18）

- 状态：`PASS`，候选 commit：`d608e946ddd3f6456ce7d7507567c8be0641cde7`。
- 完整日志：`/tmp/log-query-mcp-rc-check-d608e94.log`；末行明确为 `rc_check: PASS`。
- 环境：`rustc 1.97.1 (8bab26f4f 2026-07-14)`、`cargo 1.97.1`、Python 3.10.12、`jsonschema 4.23.0` Draft 2020-12；target `x86_64-unknown-linux-gnu`。
- 发布包：`/tmp/log-query-mcp-dist-d608e94/rc-check/log-query-mcp-v0.2.0-x86_64-unknown-linux-gnu.tar.gz`。
- 外部 archive SHA256：`cca89fd56692c7dd18dd05c690b95179e8b0591c7d3ea243bc63e199cee876e4`；包内 `SHA256SUMS` 外部文件 SHA256：`4b92f3b9a1181d6ed169afd03afcf280a9007eed96e2a15c20646eedda1008ee`。
- BUILDINFO：version `0.2.0`、commit `d608e946ddd3f6456ce7d7507567c8be0641cde7`、target `x86_64-unknown-linux-gnu`、rustc `1.97.1`。
- RC 中的 Direct live/performance 和适用的真实 WSL 检查仍保持独立门禁，未被本地 RC 的 synthetic/self-test 代签；ProxyCommand live/performance 检查转为 post-v2。

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

### 阶段 D 实际状态（2026-08-18）

- 状态：`PASS`；产品代码验证 commit：`d608e946ddd3f6456ce7d7507567c8be0641cde7`（后续仅有文档状态提交）。
- 本地 OpenSSH/SFTP fixture：`/tmp/log-query-mcp-ssh-poc-local`，严格 known_hosts、密码和加密私钥认证均实际执行。
- 通过：`ssh_transport_live` 12/12、`m4_sync_live` 1/1、`m5_remote_query_live` 1/1、`m6_remote_security_live` 1/1、`m6_multi_server_live` 3/3、`m6_concurrency_performance_live` 2/2；M6 restart 三阶段 3/3。
- 并发证据：`M6_CONCURRENCY_METRIC` dual-server 2 queries `2195ms`，single-server 4 queries `336ms`。
- 当前验证为本机真实 OpenSSH/SFTP，不是 GitHub Actions；GitHub-hosted runner 的外部 Billing 阻塞仍单独记录在阶段 A。

## 10. 阶段 E：ProxyCommand post-v2 延后门禁

ProxyCommand 已正式移出 v2.0 发布范围。本阶段保留完整的未来验证清单，供 v2.1/post-v2 重新纳入时使用；当前不作为 v2.0 RC 的 required gate，也不因 GitHub Actions Billing 或 Windows helper 缺失阻塞 v2.0。

post-v2 仍需在独立候选 SHA 上执行：

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

post-v2 完成标准：上述工作流真实执行并全部 PASS，没有孤儿 helper、敏感信息泄漏或无法解释的资源泄漏。v2.0 不宣称这些结果已通过。

### 阶段 E 实际状态（2026-08-18）

- 状态：`POST-V2 DEFERRED`；下列结果是历史本地 live 证据，不是 v2.0 required gate，也不改变本次正式 scope decision。验证基于产品代码 commit `d608e946ddd3f6456ce7d7507567c8be0641cde7`。
- scope decision 记录 commit：`b680f73`（已包含在独立 `release/v2-rc` 分支，不写入远端候选分支）；v2.0 RC 入口不再执行 post-v2 WSL evidence/manifest self-test。
- RC 入口最终清理 commit：`a02bc63`；本轮最终本地 RC 日志：`/tmp/log-query-mcp-v2-scope-rc-final.log`，末行明确为 `rc_check: PASS`。
- 通过：ProxyCommand success/strict host key 2/2；Proxy auth 2/2；Proxy sync 5/5；failure matrix 8/8；mixed query 2/2；restart 3/3；generation 1/1；helper orphan 检查 PASS。
- 失败处理：首次 mixed query 复用 Direct fixture 时缺少 `/home/logreader/logs/mixed.log`，导致两个测试出现 `SftpProtocol`；按 workflow 补充只读 `M7MIX remote` fixture 后复测 2/2 PASS。分类：测试夹具配置，不是产品代码失败。
- `/usr/bin/nc` 作为管理员预置 ProxyCommand helper，SSH 仍使用 strict known_hosts、SFTP 只读操作；未增加 Exec/Shell 或任意路径能力。
- 当前验证为本机真实 ProxyCommand→SSH→SFTP，不是 GitHub Actions；该证据保留给 post-v2，不能代替未来 post-v2 candidate workflow。

## 11. 阶段 F：Direct 性能和资源证据

v2.0 运行 Direct 性能 gate，至少采集：

- Direct 5 次 session setup latency。
- 100 MiB Full bootstrap。
- 1 GiB Full bootstrap。
- 10 GiB logical file 的 Tail 64 MiB bootstrap。
- unchanged continuity probe 传输量不超过设计上限。
- incremental append 只传输追加数据和有界探测。
- cache-local scan 不产生远程字节。
- Direct 并发查询。
- CPU、最大 RSS、elapsed time、磁盘占用和网络字节。
- 每阶段结束后的 helper orphan 检查。

入口：

```text
Direct SSH performance workflow/test and the M6/M7 Direct performance evidence
```

完成标准：Direct performance workflow PASS，指标和环境信息作为 artifact 保存，并确认没有无法解释的性能、内存、句柄、磁盘或进程回归。Proxy paired performance 留在阶段 E 的 post-v2 范围。

历史 M6 指标只能作为对照，不能替代当前 M7 候选执行。

### 阶段 F 实际状态（2026-08-18）

- 状态：`PASS（本地 Direct live/performance gate）`；历史本地证据基于产品代码 commit `d608e946ddd3f6456ce7d7507567c8be0641cde7`，但当前 v2.0 scope 变更后应在最终候选上重跑 Direct performance。
- Direct 证据：100 MiB cold `3509 ms`、1 GiB cold `29434 ms`、10 GiB logical/Tail 64 MiB cold `1747 ms`；unchanged probe 仅远程读取 `65536` bytes；local scan 远程 `0` bytes；incremental 传输和缓存写入符合有界设计。
- 资源证据：本地 profile 的 `/usr/bin/time -v`、metrics、磁盘检查和输出日志保存在 `/tmp/log-query-mcp-m7-perf-local/`；最大 RSS 约 `72 MiB`，未发现资源回归。
- Proxy paired 数字和 helper orphan 检查仍保留在历史记录中，归入 post-v2，不作为 v2.0 performance PASS。

## 12. 阶段 G：真实 Direct/WSL 验收

### 12.1 必须证明的场景

```text
WSL service identity
    +
Direct TCP -> SSH -> strict known_hosts -> auth -> SFTP
    +
list_log_sources/search_logs/get_log_context PASS
    +
direct failure/recovery and no sensitive error leakage PASS
```

### 12.2 执行入口

严格按照候选分支中的最新 Direct 说明执行；ProxyCommand/Windows helper 脚本不再是 v2.0 入口：

```text
docs/PRODUCTION_CHECKLIST.md
docs/INSTALL.md
scripts/healthcheck.sh
MCP stdio/HTTP smoke and the Direct target evidence procedure
```

不得自行简化 service identity、binary SHA、systemd 或 helper cleanup 校验。

### 12.3 必须保存的证据

- 候选 commit、binary SHA256 和 BUILDINFO。
- WSL/目标 SSH 环境描述（如部署使用 WSL）。
- Direct TCP 可达性和路由证据。
- 实际 systemd User、MainPID 和 `/proc/<pid>/exe` 对应候选 binary 的证据。
- stdio 和 Streamable HTTP MCP initialize/tools/list/三工具结果。
- Direct 失败/恢复和服务身份证据。
- strict host-key 和认证结果。
- 脱敏 evidence verifier 或等价检查结果。

证据必须脱敏，不保存 Secret、私钥、密码、完整日志正文、match_ref 或不必要的内部主机信息。

完成标准：适用的真实 stdio/HTTP Direct 查询验收 PASS，证据完整且脱敏。Windows helper/ProxyCommand evidence 不属于 v2.0 完成条件。

### 阶段 G 实际状态（2026-08-19）

- 状态：`Direct/TUN PASS；ProxyCommand POST-V2 DEFERRED`；Direct/TUN 真实目标候选链路已通过。ProxyCommand systemd HTTP gate 已从 v2.0 scope 移除，不再作为 G 的 required 条件。
- WSL 更新和重启后，当前 kernel 为 `6.18.33.2-microsoft-standard-WSL2`，PID 1 为 `systemd`，`systemctl` bus 为 `249.11`；当前 `log-query-mcp.service` 已以 `log-query-mcp` 用户运行（MainPID `42188`，`ActiveState=active`，监听 `127.0.0.1:8000/mcp`），证明 systemd/service identity 这一部分前置已生效。操作者在同一 Ubuntu-22.04 交互式 root shell 和 `log-query-mcp` 用户下均成功执行 Windows `tasklist.exe`，因此 WSL interop 已确认；但 Windows `PATH` 中 `where ncat.exe` 和 `where nc.exe` 均未找到文件，当前没有可用于 ProxyCommand 的批准 TCP helper。
- Windows 主机 `wsl --update` 已成功；`wsl --version` 报告 WSL `2.7.11.0`、kernel `6.18.33.2-2`、WSLg `1.0.73.2`、Windows `10.0.19045.6456`。原 `/opt/log-query-mcp/BUILDINFO` 仍是旧的 `v0.1.0` / `ce2abf108da6c7e8e709ddbaa3992066807f83c3`，不是本候选 `d608e946ddd3f6456ce7d7507567c8be0641cde7`；旧 `/etc/log-query-mcp/config.json` 仍保留本地目录 source，未被覆盖。用户要求切换 MCP 配置后，当前 unit 通过 `/etc/systemd/system/log-query-mcp.service.d/v2-direct.conf` 使用 v2 Direct 配置、Secret EnvironmentFile 和候选二进制；旧二进制仍保留在原路径。
- 当前 WSL 确有 `wsl-v2ray` TUN（`172.19.0.1/30`）；策略表 `2022` 通过 `172.19.0.2` 承接 `fwmark 0x2023`，而未带 mark 的普通目标路由仍经 `eth0` 默认网关。对用户提供的 Direct SSH 目标执行 TCP/SSH host-key 只读预检成功，但应用连接是否实际带 TUN mark 仍需在真实服务请求中确认。
- 已在 `/etc/log-query-mcp/config-v2-direct.json` 准备未提交到仓库的 v2 Direct 配置：密码仅引用 `QAXCICD_SDWXB_PASSWORD`，远程文件为用户提供的单文件 source，无 `proxy` 字段；`/etc/log-query-mcp/known_hosts-v2-direct` 复用本机已有受信任 host key。配置 JSON Schema PASS，候选二进制以 `log-query-mcp` 用户在隔离 loopback 端口启动 PASS。
- `/etc/log-query-mcp/secrets-v2-direct.env` 已配置 `QAXCICD_SDWXB_PASSWORD`，权限已修正为 `root:log-query-mcp 0640`。候选 v2 隔离进程以 `log-query-mcp` 用户完成 `initialize`、`tools/list`、`list_log_sources`、真实 SSH/SFTP 远程同步、`search_logs` 和 `get_log_context`：`Direct/TUN candidate MCP chain: PASS`（结果 1 条、上下文 5 行；未保存日志正文或 match_ref）。首次请求使用了不支持的 `newest_first`，按服务协议修正为 `oldest_first` 后通过，分类为测试请求错误而非产品失败。
- 在 WSL 重启确认 systemd 后，使用与现有 unit 相同的 `NoNewPrivileges`、`PrivateTmp`、`ProtectSystem=strict`、`ProtectHome`、地址族和可写 cache 限制，以临时 unit `log-query-mcp-v2-direct-check.service` 启动候选二进制；MainPID `40383`、User `log-query-mcp`、`ActiveState=active`。该 systemd 身份下 Direct/TUN MCP 链路再次 PASS：`initialize`、`tools/list`、`list_log_sources`、真实 `search_logs`（结果 1 条）和 `get_log_context`（上下文 3 行）；临时 unit 随后已停止，旧服务仍为 PID `162`。首次 503 的根因是 `PrivateTmp` 隔离导致测试二进制放在 `/tmp` 后对 unit 不可见（退出码 127），不是远端连接、密码或产品错误。
- 本次 Direct/TUN systemd 验证基于产品代码 commit `d608e946ddd3f6456ce7d7507567c8be0641cde7`；未保存 Secret、密码、完整日志正文或 `match_ref`。当前结果证明候选服务身份和 Direct SSH/SFTP 查询链路可用；应用层 TUN mark 仍按部署环境记录，不再需要 ProxyCommand helper gate。
- 用户要求将 MCP 配置切换到 75 后，已新增 v2 候选二进制 `/opt/log-query-mcp/bin/log-query-mcp-v2-direct`，并通过 systemd drop-in 将 `log-query-mcp.service` 切换到 `/etc/log-query-mcp/config-v2-direct.json`；旧 `/etc/log-query-mcp/config.json` 和旧二进制未覆盖。补充 `ReadWritePaths=/var/lib/log-query-mcp/cache-v2-direct` 后，当前 service MainPID `42188`、User `log-query-mcp`、`ActiveState=active`，监听 `127.0.0.1:8000/mcp`。75 的日志 source 已统一为 `log-75-sdwxb-base`，旧 source cache 保留未删除；从当前 8000 实际执行 `initialize`、`tools/list`、`list_log_sources`、真实 `search_logs`（结果 1 条）和 `get_log_context`（上下文 3 行）：`MCP 8000 -> 10.58.168.75 Direct/TUN chain: PASS`。
- Codex 重启后的实际 MCP 入口是 stdio，而不是上述 HTTP 端口；旧 Codex 配置曾启动 `/opt/log-query-mcp/bin/log-query-mcp-stdio` 并读取旧 `/etc/log-query-mcp/config.json`，因此只返回 `183-sdwxb-base`。已将 `/root/.codex/config.toml` 的日志 MCP 项切换为 `log-sdwxb-base-direct`、v2 stdio binary 和 `/etc/log-query-mcp/config-v2-direct.json`；密码只在启动命令中从 `/etc/log-query-mcp/secrets-v2-direct.env` 加载，未写入 Codex 配置，并保留了 `config.toml.before-log75-direct-20260819.bak`。随后将 75 的日志 source 统一命名为 `log-75-sdwxb-base`，按 Codex 同等 stdio 启动链实际验证：`list_log_sources` 返回新 source，真实 `search_logs` 和 `get_log_context` 均 PASS；PG MCP 配置未修改。
- 上述是当前 WSL service identity 下的 Direct 配置验收，不是目标 Linux 生产 MainPID 验收；原 M7 ProxyCommand 和 Windows helper gate 已转为 post-v2，不影响 v2.0。
- v2.0 后续条件：在真实目标环境按 Direct 配置完成安装、service identity、stdio/HTTP 查询、权限、故障恢复和脱敏证据验收。

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
- v2 example 为 Direct-only；ProxyCommand schema/code 不作为 v2.0 package acceptance。
- install/uninstall/healthcheck/upgrade/rollback 脚本可执行。
- M6 Release Readiness 和 v2.0 验收文档包含在包内；ProxyCommand/M7 WSL helper 文档归 post-v2。

在隔离环境执行：

- 全新安装。
- MCP protocol health check。
- 保留现有生产配置的正常升级。
- restart 或协议健康失败后的自动回滚。
- 显式手工回滚。
- 损坏包在修改系统前被拒绝。
- 回滚后服务和 MCP 协议恢复。

完成标准：package validator、健康检查和全部生命周期场景 PASS。

### 阶段 H 实际状态（2026-08-18）

- 状态：`PASS（本地 RC/package/lifecycle gate 已执行）`；产品代码 commit `d608e946ddd3f6456ce7d7507567c8be0641cde7`。
- `scripts/rc_check.sh` 已通过 package validator、healthcheck failure matrix、upgrade/rollback matrix 和包内文件/权限检查；完整日志：`/tmp/log-query-mcp-rc-check-d608e94.log`。
- RC 包：`/tmp/log-query-mcp-dist-d608e94/rc-check/log-query-mcp-v0.2.0-x86_64-unknown-linux-gnu.tar.gz`；外部 SHA256：`cca89fd56692c7dd18dd05c690b95179e8b0591c7d3ea243bc63e199cee876e4`。包内 `BUILDINFO` 已固定版本 `0.2.0`、target 和候选 commit。
- 该状态只覆盖隔离本地 package/lifecycle gate；未对生产目标执行安装、升级、重启或回滚，也未替代目标 Linux/WSL 验收。

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
- Local、Direct 和 Local+Direct 混合查询。
- 日志轮转、SSH 中断、权限变化、缓存损坏和恢复。
- 实际 AI 客户端调用。
- 升级和回滚演练。

完成标准：发布必需项有操作者、时间和证据；未执行项保持“待验收”，不得由 CI 代签。

### 阶段 I 实际状态（2026-08-19）

- 状态：`待验收 / BLOCKED-ENV`；当前 WSL 的 systemd service bus 已在重启后确认正常，但没有可供本轮使用的目标 Linux 预生产/生产环境和操作者批准的生产安装、升级、重启或回滚入口。
- 已保留本地 RC、Direct live 和性能证据；这些证据不能代签目标内核/glibc、systemd、权限、轮转、中断恢复、实际 AI 客户端、升级/回滚等生产项目。ProxyCommand 历史证据不属于 v2.0 release gate。
- 在获得目标环境和明确授权前，不执行安装、重启、升级、回滚或真实配置修改。

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

### 阶段 J 实际状态（2026-08-19）

- 状态：`BLOCKED / 未进入发布操作`；阶段 A、G、I 尚未完成，GitHub runner/Billing、Issue #27 发布保护以及目标 Linux 生产验收仍是硬门禁，不能直接创建 tag/release 或部署。ProxyCommand helper/Issue #26 不再是 v2.0 硬门禁，已转为 post-v2 跟踪。
- 独立 `release/v2-rc` 已推送并创建 Draft PR #30，当前 head=`7a39458`、base=`feat/v2-m1-backend-config`；不修改 `main` 或远端候选分支，不执行合并/tag/release/deploy。

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
| Performance | 当前候选 Direct metrics 和 artifact 完整，无未解释回归 |
| Direct/WSL | 适用的真实 stdio/HTTP Direct evidence PASS |
| Package | checksum/BUILDINFO/package validator PASS |
| Lifecycle | install/health/upgrade/rollback PASS |
| Production | 目标 Linux 和实际 AI 客户端验收 PASS |
| PR | reviewed、merged，merge commit 可追溯 |
| Release | 新 tag 和 GitHub Release 已发布并复验 |

以下状态均不能称为“发布完成”：

- 代码已写但 live tests 未执行。
- workflow 因 Billing 在 runner 启动前失败。
- 只有历史 M6 证据，没有当前 Direct 候选证据。
- 只有 WSL 静态检查，没有适用的真实 Direct/SSH 链路。
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
