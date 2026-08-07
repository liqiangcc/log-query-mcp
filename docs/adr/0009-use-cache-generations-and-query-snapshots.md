# ADR-0009：Remote Cache 使用 Generation 与查询快照保证一致性

- 状态：Proposed for v2
- 日期：2026-08-07

## 决策

1. 每个远程日志文件在本地 Cache 中按 generation 保存，不直接覆盖仍可能被查询状态引用的旧副本。
2. 每次 `search_logs` 在查询开始前冻结本地 Snapshot，至少绑定 `source_id`、`file_id`、generation 和 snapshot length。
3. `cursor` 和 `match_ref` 绑定具体 generation；远程日志随后追加或轮转不会改变既有 token 的语义。
4. 远程文件 append 在连续性验证通过时写入当前 generation；truncate、replacement、连续性校验失败或无法安全确认文件身份时创建新 generation。
5. Cache Manifest 使用原子写入（临时文件 + 校验 + rename）；同步中断或进程崩溃不能破坏最后一次有效缓存状态。
6. Cache GC 只能删除已经过期且未被活动 Query Snapshot、cursor 或 `match_ref` 引用的 generation。
7. 仅依赖 size/mtime 不足以确认 append 连续性；实现必须维护并验证有限的 continuity fingerprint 或等价机制。

## 原因

远程日志会持续追加、轮转、截断和替换。如果直接用一个本地文件覆盖同步结果，分页 cursor 和 `match_ref` 可能在请求之间指向不同内容，导致上下文错误甚至返回错误证据。

Generation + Snapshot 可以把远程不断变化的日志转化为查询期间稳定的本地文件视图，并与 v1 的有状态 token 模型保持一致。

## 后果

- CacheStore 需要管理多代文件、引用计数/租约和 GC。
- 磁盘使用会短期高于单份缓存，需要全局和来源级容量上限。
- 日志轮转后旧 generation 不立即删除，而是至少保留到所有相关 token 失效。
- Query Engine 可以继续基于稳定本地文件工作，不需要处理远程并发变化。
