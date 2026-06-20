# M2 正式核心实现状态

## 已完成切片一：版本化配置模型

- Rust 2024 正式工程初始化。
- 应用层禁止 `unsafe`。
- `AppConfig`、日志来源、目录规则、时间戳规则和资源限制模型。
- JSON 未知字段拒绝。
- v1 默认值和代码硬上限。
- source_id、文本长度、重复项和路径规范校验。
- 资源限制跨字段关系校验。
- 从字符串和文件加载配置。
- 提交 `Cargo.lock`，CI 使用 `--locked`。

## 已完成切片二：安全来源注册表

- `SourceRegistry` 只注册启用来源。
- `SafeRoot` 持有管理员配置的来源 root FD。
- 文件和目录使用 `openat2()` 从 root FD 解析。
- 启用 `RESOLVE_BENEATH`、`RESOLVE_NO_SYMLINKS`、`RESOLVE_NO_MAGICLINKS` 和 `RESOLVE_NO_XDEV`。
- 打开后使用 `fstat`，只接受普通文件或目录。
- 启动时验证显式文件和目录规则。
- 目录发现按后缀、递归选项、文件数、目录数和目录项数实施硬限制。
- 软链接、特殊文件和非 UTF-8 发现项不会进入候选集合。
- 查询可创建带 device、inode 和大小的文件快照。
- 文件替换或截断后，旧快照安全失效。
- 内部路径仍必须属于显式文件或目录规则，不能仅依赖 root 边界。
- Rustfmt、Clippy、单元测试和 Contracts CI 全部通过。

详细说明见 [`M2_SOURCE_REGISTRY.md`](./M2_SOURCE_REGISTRY.md)。

## 当前不包含

- MCP Server 和工具 Schema 的 Rust 类型实现。
- 有界日志扫描器。
- 多文件和多来源查询编排。
- `match_ref`、cursor 和上下文读取。
- 具备 mount 权限环境中的真实 `RESOLVE_NO_XDEV` 跨挂载集成测试。

## 下一切片

```text
有界流式扫描器
+ ScanExecutor
+ deadline / cancellation
+ 单文件搜索结果模型
```
