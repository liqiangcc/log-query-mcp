# M2 SourceRegistry / SafeRoot 评审清单

## 安全文件访问

- [x] 来源 root 由管理员配置，客户端不能提交路径。
- [x] 文件和目录从持有的 root FD 相对打开。
- [x] 使用 `openat2()`。
- [x] 启用 `RESOLVE_BENEATH`。
- [x] 启用 `RESOLVE_NO_SYMLINKS`。
- [x] 启用 `RESOLVE_NO_MAGICLINKS`。
- [x] 启用 `RESOLVE_NO_XDEV`。
- [x] 文件打开后只接受普通文件。
- [x] 目录打开后只接受目录。
- [x] FIFO、Socket、软链接和路径穿越测试已覆盖。

## 目录发现

- [x] 只使用管理员配置的目录和后缀。
- [x] 支持非递归和可选递归。
- [x] 不跟随软链接。
- [x] `d_type` 未知时重新经过安全打开分类。
- [x] 目录、目录项和匹配文件数量有硬上限。
- [x] 输出稳定排序并去重。

## SourceRegistry

- [x] 只加载启用来源。
- [x] 没有启用来源时拒绝启动。
- [x] 启动时验证显式文件。
- [x] 公开描述不包含 root 和服务器路径。
- [x] `file_id` 不作为路径使用。
- [x] 每次实际打开文件重新执行安全验证。
- [x] 未知来源和未知文件有明确内部错误。

## 自动验证

- [x] Contracts CI 通过。
- [x] Rustfmt 通过。
- [x] Clippy `-D warnings` 通过。
- [x] Rust 单元测试通过。

## 后续切片

- [ ] 查询时刷新目录候选快照以覆盖服务启动后的日志轮转。
- [ ] 增加真实嵌套 mount / bind mount 集成测试。
- [ ] 接入有界扫描器和查询编排。
