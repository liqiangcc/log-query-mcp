# M2 正式核心实现状态

## 已完成切片

### 1. 版本化配置模型

- Rust 2024 正式工程初始化。
- 应用层禁止 `unsafe`。
- `AppConfig`、日志来源、目录规则、时间戳规则和资源限制模型。
- JSON 未知字段拒绝。
- v1 默认值、代码硬上限和跨字段关系校验。
- 从字符串和文件加载配置。
- `Cargo.lock` 与 `--locked` CI。

### 2. SourceRegistry 与安全文件边界

- `SafeRoot` 持有管理员配置的来源根目录 FD。
- 文件和目录通过 Linux `openat2()` 相对打开。
- v1 启用：
  - `RESOLVE_BENEATH`
  - `RESOLVE_NO_SYMLINKS`
  - `RESOLVE_NO_MAGICLINKS`
  - `RESOLVE_NO_XDEV`
- 打开后通过 `fstat` 只接受普通文件或目录。
- 拒绝路径穿越、绝对路径、软链接、FIFO、Socket 和替换后的软链接。
- 有界目录发现支持后缀白名单、可选递归、稳定排序和去重。
- 限制目录规则数、遍历目录数、目录项数和发现文件数。
- `SourceRegistry` 只暴露来源描述和不透明 `file_id`，不暴露来源 root。
- 启动时验证显式文件并加载发现结果。
- 每次真正打开配置文件时重新经过 `SafeRoot` 校验。

## 自动验证

```text
Contracts CI: passed
cargo fmt --check: passed
cargo clippy --locked -D warnings: passed
cargo test --locked: passed
```

## 当前边界

- 当前目录规则在 `SourceRegistry` 构建时形成文件集合；查询阶段的轮转文件重新发现将在多文件查询编排切片中完成。
- 尚未执行真实嵌套 mount 测试；当前代码和单元测试确认 `RESOLVE_NO_XDEV` 已进入解析策略。
- 尚未实现日志流式扫描、MCP Server、时间过滤、cursor、match_ref 和上下文读取。

## 下一切片

```text
有界单文件扫描器
+ ScanExecutor
+ deadline / cancellation
+ 扫描结果位置模型
```
