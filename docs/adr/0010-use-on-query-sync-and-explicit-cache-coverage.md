# ADR-0010：v2 默认按查询同步，并显式表达缓存覆盖范围

- 状态：Accepted for v2
- 日期：2026-08-07

## 决策

1. v2 MVP 的 Remote Source 默认采用 `on_query` 新鲜度策略：`search_logs` 在建立查询 Snapshot 前检查远程变化，并仅在必要时增量同步。
2. v2 MVP 不要求后台周期同步；后台同步作为后续优化，不改变 MCP 查询接口。
3. Remote Source 必须显式配置 Bootstrap 策略：
   - `full`：首次完整同步；
   - `tail`：首次只同步最近 N bytes；
   - `from_now`：首次只记录当前位置，后续只同步新增内容。
4. Bootstrap 策略决定缓存覆盖范围。若请求可能需要访问未缓存历史，服务必须返回 `CACHE_SCOPE_EXCEEDED` 或等价稳定错误，不能返回空结果制造假阴性。
5. 默认 `allow_stale_on_error=false`。如果查询要求刷新而 SSH/SFTP 不可用，必须明确返回 Remote/Sync 错误，不得静默查询旧缓存。
6. 未来若支持 `allow_stale_on_error=true`，响应必须显式携带 stale/coverage 元数据；在该元数据进入正式 MCP 契约之前，不允许开启该行为。
7. 增量同步只下载已确认的新增范围；不能把“每次完整下载”作为正常查询路径。

## 原因

日志排障对“没有找到”非常敏感。如果缓存覆盖范围不完整或缓存已经过期，却仍返回空结果，AI 会把数据缺失误判为业务事实。显式 Bootstrap、Coverage 和 Freshness 语义可以优先保证正确性，再优化速度。

`on_query` 同步也比首期后台同步更容易控制资源和故障边界，同时能覆盖大多数开发/测试排障场景。

## 后果

- 首次查询的延迟取决于 Bootstrap 策略和日志规模。
- `tail` / `from_now` 模式不能保证任意历史查询完整，需要查询层执行 Coverage 检查。
- SSH 不可用时默认查询失败，而不是自动降级到旧缓存。
- 后续增加后台同步时仍需复用相同 Cache Manifest、Generation 和 Coverage 语义。
