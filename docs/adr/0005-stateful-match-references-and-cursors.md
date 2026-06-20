# ADR-0005：match_ref 和 cursor 使用服务端有状态随机 token

- 状态：Accepted for single-instance v1
- 日期：2026-06-20

## 背景

客户端需要：

- 根据搜索结果读取有限上下文。
- 在结果截断后继续下一页。

但客户端不能获得服务器路径、任意行号、字节偏移或候选文件列表。

候选方案包括服务端有状态随机 token 和签名无状态 token。

## 决策

首期采用单实例、服务端内存状态：

```text
match_ref -> source/file identity/match position
cursor    -> query/candidate snapshot/scan position/budget
```

客户端只获得 UUID v4 随机 token。

两类 Store 均具有：

- TTL。
- 容量上限。
- 最旧项淘汰。
- 格式和查询绑定校验。
- 服务重启后失效。

## 原因

- 不向客户端编码路径、inode 或偏移。
- 实现简单，便于安全审计。
- 查询条件和文件快照可以完整绑定。
- 适合当前单实例、短生命周期的问题排查场景。
- 无需引入签名密钥、token 版本和外部数据库。

## 后果

正面：

- 客户端无法构造任意位置读取请求。
- 文件轮转、替换和截断可以使引用安全失效。
- 分页可以累计扫描资源配额。

负面：

- 服务重启后 token 失效。
- 多实例需要粘性会话或共享 Store。
- Store 需要容量和清理监控。

## 约束

- 内部状态类型不得直接序列化到 MCP 响应。
- get_log_context 必须重新经过 SourceRegistry 和 SafeRoot。
- cursor 必须绑定规范化查询条件。
- 服务端错误不区分未知、过期和已淘汰 token 的内部细节。
- 首期部署保持单实例；多实例设计另立 ADR。
