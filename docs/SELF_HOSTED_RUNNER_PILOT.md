# Ephemeral Self-hosted Runner Pilot

## 目的

只验证一个问题：当前 GitHub-hosted runner 被 Billing / spending-limit 阻塞时，repository self-hosted runner 是否仍能真正执行 GitHub Actions step。

这个 Pilot 不替换正式 CI，不运行生产任务，也不携带生产 Secret。

## 安全边界

运行机器 SHOULD：

- 是临时或专用于该 Pilot 的 Linux x86_64 主机；
- 不存放生产凭证；
- 不挂载生产日志或生产配置；
- 使用普通非 root 用户；
- 仅具备 GitHub 和依赖下载所需的出站网络；
- Pilot 完成后停止/删除 runner 工作目录。

Workflow 只匹配：

```text
self-hosted + verification-pilot
```

避免普通 self-hosted runner 意外消费该探针。

## 1. 获取临时注册信息

进入仓库：

```text
Settings
→ Actions
→ Runners
→ New self-hosted runner
→ Linux / x64
```

从页面获取：

- 临时 registration token；
- GitHub 官方 runner Linux x64 下载 URL。

不要把 token 提交到仓库、Issue、PR 或 shell history。

## 2. 启动 ephemeral runner

在隔离 Linux 主机 clone `feat/verification-pilot` 后：

```bash
read -rsp 'Runner token: ' RUNNER_TOKEN; echo
export RUNNER_TOKEN
export RUNNER_PACKAGE_URL='<copy official Linux x64 runner archive URL from GitHub Settings>'

bash scripts/bootstrap-self-hosted-runner.sh
```

脚本固定：

```text
--ephemeral
--labels verification-pilot
RUNNER_WORK_DIR=/tmp/log-query-mcp-actions-runner
```

因此 runner 只处理一个匹配 Job，完成后退出。

## 3. 成功证据

当前 PR #29 的 `Self-hosted Billing Probe` 必须真正执行并输出：

```text
SELF_HOSTED_BILLING_PROBE
repo=liqiangcc/log-query-mcp
sha=<candidate sha>
runner=<runner name>
```

仅显示 `queued` 不算成功。

## 4. 失败分类

```text
ACCOUNT_LOCK_BLOCKS_SELF_HOSTED
RUNNER_NOT_REGISTERED
RUNNER_OFFLINE
LABEL_MISMATCH
WORKFLOW_PERMISSION
NETWORK_OR_DEPENDENCY_FAILURE
OTHER_GITHUB_CONTROL_PLANE
```

如果 Job 没有任何实际 step，不得分类为代码或 Convention failure。

## 5. 探针通过后的下一步

不要立即把全部 CI 切到 self-hosted。

按顺序验证：

```text
Probe
→ ./scripts/verify conventions
→ ./scripts/verify contracts
→ ./scripts/verify rust
```

每一步都必须使用同一 candidate SHA，并保留真实 step 日志。

## 6. 清理

runner 是 ephemeral，完成一个 Job 后进程应退出。

确认 runner 不再运行后删除：

```bash
rm -rf /tmp/log-query-mcp-actions-runner
```

注册 token 本身是临时凭证；不要复用、持久化或写入环境配置文件。
