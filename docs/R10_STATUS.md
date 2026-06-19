# R-10 部署与性能验证状态

已完成：

- 真实 MCP 查询链路集成。
- Linux JSON 配置示例。
- systemd 加固 unit 示例。
- `SIGTERM` 优雅停止。
- 确定性日志生成脚本。
- 单文件扫描基准二进制。
- CI 中 1 MiB 完整扫描烟测。
- Linux 部署和性能基准执行文档。

当前 CI 结果：

```text
rustfmt: passed
Clippy -D warnings: passed
unit/integration tests: 78 passed
benchmark smoke: passed
```

CI 烟测只验证基准链路可运行，不代表生产性能结论。1 GiB、10 GiB、多文件和并发数据仍需在目标 Linux 服务器采集。
