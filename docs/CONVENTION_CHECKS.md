# Convention Checks

本项目的第一版 Convention Checker 只检查高确定性、无需理解 Rust 业务语义的结构约定。

运行：

```bash
./scripts/verify conventions
```

实现：

```text
scripts/check_conventions.py
```

## Rules

| Rule | Meaning |
|---|---|
| `LQM_HTTP001` | 必须存在 `tests/http/mcp/initialize.http` 这个生产安全的 MCP 初始化验证资产 |
| `LQM_HTTP002` | 每个 `.http` Case 必须有唯一 `# @name` |
| `LQM_HTTP003` | `production-safe` Case 不得同时标记 `destructive` |
| `LQM_HTTP004` | HTTP 请求必须通过环境 `BASE_URL` 定位实例，不能把部署地址写死在测试资产中 |
| `LQM_HTTP005` | 每个 HTTP Case 必须至少断言一个明确 HTTP status |

这些编号使用 `LQM_` 前缀，表示当前是项目本地 Pilot 规则。它们不会冒充 `ai-engineering-conventions` 中尚未在本项目采用的通用 `UC` / `BR` 规则。

## Separation of concerns

```text
Convention Checker
→ 检查测试资产结构、名称和安全元数据

httpYac
→ 真正执行 HTTP 请求和断言

scripts/verify
→ 提供稳定编排入口

GitHub Actions
→ 远程调用相同入口
```

Checker 不复制 MCP `initialize` 的业务/协议判断；协议正确性仍由 `.http` Case 与现有 MCP contract tests 负责。

## CI

`.github/workflows/conventions.yml` 只执行：

```bash
./scripts/verify conventions
```

因此未来 Checker 的具体实现可以变化，而 CI 编排入口保持稳定。

## Current verification status

当前 GitHub-hosted Actions 因账户 billing / spending-limit 在 runner 启动前失败，因此：

```text
Checker implementation: IMPLEMENTED
CI wiring: IMPLEMENTED
CI runtime: NOT VERIFIED / TEST_ENVIRONMENT_FAILURE
```

不得把 `steps=[]` 的 Actions failure 解释为 Convention failure。
