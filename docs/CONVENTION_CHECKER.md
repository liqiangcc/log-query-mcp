# Convention Checker

## 目的

Convention Checker 只检查当前仓库已经稳定、客观、可机器判断的约定，不判断业务规则是否正确。

执行入口：

```bash
./scripts/verify conventions
```

该入口分成两个关注点：

```text
conventions-test
→ 证明 Checker 自身规则判断没有回归

convention-checker
→ 用 Checker 检查真实仓库资产
```

只运行 Checker 自测：

```bash
./scripts/verify conventions-test
```

## 当前规则

| Rule | Meaning |
|---|---|
| `LQM_HTTP001` | 必须存在 `tests/http/mcp/initialize.http` |
| `LQM_HTTP002` | 每个 HTTP Case 必须声明唯一 `# @name` |
| `LQM_HTTP003` | `production-safe` 不能同时标记 `destructive` |
| `LQM_HTTP004` | HTTP 请求必须通过环境 `BASE_URL` 选择目标实例 |
| `LQM_HTTP005` | 每个 HTTP Case 必须断言明确的 HTTP status |

这里使用 `LQM_*` 项目本地 Rule Code，而不是直接套用通用 UC/BR/operation 规则。原因是当前仓库尚未引入完整的业务 Use Case / Business Rule metadata，Pilot 不应先造一套并不存在的事实模型。

## Checker Self Tests

测试位置：

```text
tests/scripts/test_check_conventions.py
```

使用临时目录 fixture，不依赖真实网络、Rust build 或运行服务。

覆盖：

```text
valid asset                 → PASS
missing initialize          → LQM_HTTP001
duplicate @name             → LQM_HTTP002
production-safe destructive → LQM_HTTP003
hard-coded base URL         → LQM_HTTP004
missing status assertion    → LQM_HTTP005
```

Checker 的规则判断通过：

```python
collect_violations(root)
```

暴露为可测试能力；CLI 只负责输出和 exit code。

## 失败输出

示例：

```text
[LQM_HTTP004] tests/http/mcp/initialize.http: HTTP request must use the BASE_URL environment variable
[conventions] FAIL (1 violation(s))
```

目标是让开发者和 AI 不需要重新全仓搜索就能知道：

```text
哪条约定失败
哪个文件失败
应该修复什么
```

## 与其他验证能力的边界

```text
Convention Checker
→ 结构 / metadata / HTTP 测试资产完整性

Checker Self Tests
→ Checker 自身实现可靠性

Rust tests
→ Rust 行为

Contract validation
→ JSON Schema / API Contract

httpYac
→ 真实运行实例 HTTP 行为
```

因此 Checker 不做：

- Rust AST 分析；
- MCP 业务语义判断；
- JSON Schema 重复验证；
- 真实 HTTP 请求；
- 部署状态检查。

## CI

独立 Workflow：

```text
.github/workflows/conventions.yml
```

只调用：

```bash
./scripts/verify conventions
```

Workflow 不复制 Checker Rule，实现和规则测试继续留在 `scripts/` 与 `tests/scripts/`。

## 当前 Pilot 状态

Checker、Self Tests 和 CI wiring 已实现，但 GitHub Actions 当前因 Billing / spending-limit 在 runner 启动前失败，Job 显示 `steps=[]`。

因此当前状态是：

```text
Implementation: IMPLEMENTED
Runtime: NOT VERIFIED
Failure class: TEST_ENVIRONMENT_FAILURE
```

不能因为 GitHub Check 显示红色就归类为 `CONVENTION_FAILURE`。
