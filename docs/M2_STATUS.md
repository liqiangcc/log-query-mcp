# M2 正式核心实现状态

## 当前切片：版本化配置模型

已完成：

- Rust 2024 正式工程初始化。
- 应用层禁止 `unsafe`。
- `AppConfig`、日志来源、目录规则、时间戳规则和资源限制模型。
- JSON 未知字段拒绝。
- v1 默认值和代码硬上限。
- source_id、文本长度、重复项和路径规范校验。
- 资源限制跨字段关系校验。
- 从字符串和文件加载配置。
- 提交 `Cargo.lock`，CI 使用 `--locked`。
- Rustfmt、Clippy、单元测试和 Contracts CI。

当前不包含：

- 来源 root 的系统级打开。
- 显式文件和目录规则的运行时发现。
- `openat2()`、`RESOLVE_NO_XDEV` 和普通文件校验。
- MCP Server 和查询链路。

下一切片：

```text
SourceRegistry
+ SafeRoot
+ 显式文件验证
+ 受控目录发现
```
