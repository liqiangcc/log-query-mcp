# M1 v1 契约评审清单

## 范围

- [x] v1 只支持 `oldest_first`。
- [x] `newest_first` 后置且请求返回 `INVALID_ARGUMENT`。
- [x] 时间范围固定为 `[start_time, end_time)`。
- [x] 不区分大小写仅保证 ASCII。
- [x] 无时间或畸形时间的匹配保守返回。
- [x] 单实例、内存 cursor 和 `match_ref`，重启后失效。

## MCP API

- [x] 三个工具的请求和成功响应已定义。
- [x] 请求拒绝未知字段。
- [x] 客户端不能提交服务器路径。
- [x] 无匹配不是工具错误。
- [x] 工具错误 wire format 和机器 Schema 已定义。
- [x] Contracts CI 验证示例和负向用例。

## 配置

- [x] `version=1`。
- [x] 来源、显式文件、目录规则和时间戳规则已定义。
- [x] 文件和目录路径只能由管理员配置。
- [x] 资源默认值和硬上限已定义。
- [x] 关键跨字段关系由校验脚本验证。

## 文件安全

- [x] Linux kernel >= 5.6。
- [x] `openat2()` 为唯一正式打开路径。
- [x] 启用 `RESOLVE_BENEATH`。
- [x] 启用 `RESOLVE_NO_SYMLINKS`。
- [x] 启用 `RESOLVE_NO_MAGICLINKS`。
- [x] v1 启用 `RESOLVE_NO_XDEV`。
- [x] 打开后只接受普通文件。

## 文档一致性

- [x] `REQUIREMENTS.md` 已同步到 v1.1。
- [x] README 已同步当前阶段。
- [x] 实现基线、API、配置和错误模型相互链接。
- [x] ADR-0001 至 ADR-0006 已进入正式分支。

## 评审后动作

- [ ] 人工确认范围和默认限制。
- [ ] 合并 PR #2。
- [ ] 从更新后的 `main` 创建 M2 核心实现分支。
- [ ] 按正式 Schema 迁移预研代码，而不是整体复制 spike 分支。
