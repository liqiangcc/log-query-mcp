# HTTP Verification

## 目的

HTTP 验证证明真实运行实例的外部 MCP 行为，不替代 Rust、Contract 或 Convention Checker。

## 可执行资产

```text
tests/http/mcp/initialize.http
```

该 Case：

```text
smoke
deployment
production-safe
```

使用环境变量选择目标实例：

```text
{{$processEnv BASE_URL}}
```

并显式断言 HTTP status。

这些结构约定由 `./scripts/verify conventions` 自动检查，但 Checker 不会真的发送 HTTP 请求。

## 运行

```bash
BASE_URL=http://127.0.0.1:8000 ./scripts/verify http-smoke
```

要求本地存在 httpYac。

## 分离点

```text
./scripts/verify conventions
→ 检查 HTTP 测试资产结构和安全 metadata

./scripts/verify http-smoke
→ 对真实服务执行 HTTP 请求和断言
```

所以：

```text
Convention PASS
≠ HTTP Runtime PASS
```

反过来 HTTP 请求成功也不能证明所有 Convention Rule 都满足。

## 使用阶段

### 本地/测试环境

启动或连接真实 `log-query-mcp` 后运行 `http-smoke`。

### Staging

部署后验证运行版本，然后对 staging `BASE_URL` 重放同一个 Case。

### Production

只允许明确标记 `production-safe` 且无破坏性的 Case 自动运行。当前 initialize 只是协议握手，不修改日志或业务数据。

## Bug Regression

未来如果真实 HTTP Bug 可以通过 `.http` 稳定复现：

```text
Bug
→ 新增/扩展正确行为断言
→ Before Fix: FAIL
→ Fix
→ After Fix: PASS
→ 永久保留
→ staging 部署后重放
```

不要在修复后删除复现 Case。

## 当前 Pilot 状态

HTTP asset 和 Runner 入口已经实现，但当前没有同时满足“可访问运行实例 + httpYac”的执行环境，因此：

```text
HTTP Runtime: NOT VERIFIED
```

不能因为 `.http` 文件存在就写 PASS。
