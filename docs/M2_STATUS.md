# M2 正式核心实现状态

## 已完成切片 1：版本化配置模型

- Rust 2024 正式工程初始化。
- 应用层禁止 `unsafe`。
- `AppConfig`、日志来源、目录规则、时间戳规则和资源限制模型。
- JSON 未知字段拒绝。
- v1 默认值、代码硬上限和跨字段关系校验。
- source_id、文本长度、重复项和路径规范校验。
- 从字符串和文件加载配置。
- 提交 `Cargo.lock`，CI 使用 `--locked`。

## 已完成切片 2：SourceRegistry 与文件安全边界

- 使用 `SafeRoot` 持有管理员批准的来源 root 文件描述符。
- 普通文件和目录均通过 `openat2()` 相对 root 解析。
- v1 解析标志：
  - `RESOLVE_BENEATH`
  - `RESOLVE_NO_SYMLINKS`
  - `RESOLVE_NO_MAGICLINKS`
  - `RESOLVE_NO_XDEV`
- 打开后通过 `fstat` 只接受普通文件或目录。
- 显式文件在注册表构建时重新安全打开。
- 目录规则采用 fd-relative 有界发现，不跟随软链接。
- 限制目录项、目录和匹配文件数量。
- 合并显式文件与发现文件，去重并稳定排序。
- 生成来源内稳定、不包含绝对路径的 `file_id`。
- 禁用来源不打开、不列出。
- `SourceRegistry` 支持列出、按 ID 获取和保持请求顺序选择来源。
- Rustfmt、Clippy、单元测试和 Contracts CI 通过。

v1 当前采用**启动时文件集合快照**：配置重启后重新发现目录文件；运行期间新增的轮转文件不会自动加入，已有相对路径仍会在后续查询中重新安全打开。若目标环境要求无重启发现新轮转文件，将在查询编排切片增加受限刷新机制。

## 当前不包含

- 特权环境中的嵌套 mount / bind mount 实际拒绝测试；当前由解析标志断言覆盖，目标 Linux 验收补充。
- 日志流式扫描和 `ScanExecutor`。
- MCP Server 和查询编排。
- cursor、`match_ref` 和上下文读取。
- 时间过滤。

## 下一切片

```text
有界流式扫描器
+ ScanExecutor
+ CancellationToken
+ deadline
+ 结果/内容限制
```
