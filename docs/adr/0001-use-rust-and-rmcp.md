# ADR-0001：使用 Rust 和官方 rmcp SDK

- 状态：Accepted for formal implementation
- 日期：2026-06-20

## 背景

服务需要长期运行在 Linux 服务器上，执行大文件扫描、并发控制、取消、文件描述符管理和严格路径边界。候选语言包括 Go、Rust、Python 和 TypeScript。

## 决策

正式实现首选：

```text
Rust stable
+ 官方 rmcp 1.x
+ Tokio
+ Axum
```

提交 `Cargo.lock`，SDK 升级必须通过协议烟测、安全测试和集成测试。

## 原因

- 无 GC，内存和延迟更可预测。
- 所有权模型适合文件描述符和短期状态生命周期。
- `Send` / `Sync` 有助于约束并发状态。
- Linux 系统调用生态满足 `openat2()` 需求。
- rmcp 已覆盖本项目实际使用的 tools-only、stdio 和 Streamable HTTP 子集。
- 当前原型和独立 MCP 协议烟测已通过。

## 后果

正面：

- 适合受限资源和长期运行的系统服务。
- 单二进制部署。
- 安全文件访问与扫描器可以保持在同一语言和类型系统中。

负面：

- 团队必须具备异步 Rust 和 Linux 系统编程能力。
- rmcp 当前完整协议成熟度低于部分 Tier 1 SDK。
- 编译时间和开发门槛高于 Go/Python。

## 约束

- 应用层启用 `#![forbid(unsafe_code)]`。
- 不直接依赖 SDK 仓库主分支。
- 升级依赖时运行 stdio 和 Streamable HTTP 协议烟测。
- 如目标客户端出现无法规避的兼容问题，重新评估 Go 官方 SDK。
