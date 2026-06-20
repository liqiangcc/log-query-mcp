# ADR-0001：使用 Rust 和官方 rmcp SDK

- 状态：Accepted
- 日期：2026-06-20

## 决策

正式实现使用：

```text
Rust stable
rmcp 1.x
Tokio
Axum
```

提交 `Cargo.lock`，SDK 和协议依赖升级必须通过 stdio、Streamable HTTP、Schema、安全和集成回归测试。

## 原因

- 资源生命周期和文件描述符所有权明确。
- 无 GC，适合有界内存和稳定延迟的日志扫描服务。
- 并发状态可通过类型系统约束。
- Linux `openat2()` 生态成熟。
- rmcp 已覆盖 v1 所需的 tools-only、stdio 和 Streamable HTTP 子集。

## 后果

- 团队需要维护异步 Rust 和 Linux 系统编程代码。
- 应用层启用 `#![forbid(unsafe_code)]`。
- 若实际目标客户端出现无法规避的协议兼容问题，再重新评估 Go 官方 SDK。
