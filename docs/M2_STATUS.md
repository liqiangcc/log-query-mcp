# M2 正式核心实现状态

## 已完成切片一：版本化配置模型

- Rust 2024 正式工程初始化。
- 应用层禁止 `unsafe`。
- 版本化来源、目录、时间戳和资源限制配置。
- JSON 未知字段拒绝、默认值、硬上限和跨字段校验。
- `Cargo.lock` 与 `--locked` CI。

## 已完成切片二：安全来源注册表

- `SourceRegistry` 只注册启用来源。
- `SafeRoot` 持有来源 root FD。
- 文件和目录通过 `openat2()` 相对解析。
- 启用 `RESOLVE_BENEATH`、`NO_SYMLINKS`、`NO_MAGICLINKS` 和 `NO_XDEV`。
- 有界目录发现和 device/inode/size 文件快照。
- 文件替换或截断后旧快照失效。

详细说明见 [`M2_SOURCE_REGISTRY.md`](./M2_SOURCE_REGISTRY.md)。

## 已完成切片三：有界扫描器与执行器

- 基于 `Read` 的流式字面量扫描。
- KMP 匹配跨读取缓冲区但不跨日志行。
- 中文 UTF-8、ASCII 大小写折叠、非法 UTF-8 和 CRLF。
- 行号、行首偏移、匹配偏移和原始行长度。
- 扫描字节、结果、单行、返回内容和缓冲区限制。
- `CancellationToken` 和绝对 deadline 协作检查。
- `ScanExecutor` 使用 Semaphore 和 `spawn_blocking`。
- 排队取消、排队 deadline、运行中取消和 Future 中止测试。

详细说明见 [`M2_SCANNER_EXECUTOR.md`](./M2_SCANNER_EXECUTOR.md)。

## 自动验证

```text
Contracts CI: passed
cargo fmt --check: passed
cargo clippy --locked -D warnings: passed
cargo test --locked: passed
```

## 当前不包含

- 多文件和多来源查询编排。
- 日志时间戳解析与时间过滤。
- MCP Server 和工具类型。
- `match_ref`、cursor 和上下文读取。
- 客户端断连到取消令牌的端到端验证。
- 具备 mount 权限环境中的真实 `RESOLVE_NO_XDEV` 集成测试。

## 下一切片

```text
多来源候选快照
+ 多文件顺序扫描
+ 时间过滤
+ oldest_first 稳定排序
+ 页级累计资源限制
```
