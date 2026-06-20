# ADR-0005：match_ref 和 cursor 使用服务端有状态随机 token

- 状态：Accepted for single-instance v1
- 日期：2026-06-20

## 决策

v1 使用服务端内存状态：

```text
match_ref -> 来源、相对路径、文件身份、匹配位置
cursor    -> 查询条件、候选快照、扫描位置、累计资源
```

客户端只获得随机 UUID token。两类 Store 均配置 TTL、容量上限和最旧项淘汰，服务重启后失效。

## 原因

- 不向客户端编码路径、inode 或偏移。
- 查询条件和文件身份可以完整绑定。
- 实现简单、易于审计，符合短生命周期单实例排查场景。
- 不需要签名密钥、token 版本或外部数据库。

## 后果

- 多实例需要粘性会话或共享 Store，v1 不支持。
- get_log_context 必须重新通过 SourceRegistry 和 SafeRoot。
- cursor 必须绑定完整规范化查询条件。
- 客户端遇到失效 token 时重新搜索。
