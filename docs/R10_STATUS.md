# R-10 部署与性能验证状态

> 总体预研结论：**CONDITIONAL GO**  
> 完整结论见 [TECHNICAL_RESEARCH_REPORT.md](./TECHNICAL_RESEARCH_REPORT.md)

已完成：

- 真实 MCP 查询链路集成。
- Linux JSON 配置示例。
- systemd 加固 unit 示例。
- `SIGTERM` 优雅停止。
- 确定性日志生成脚本。
- 单文件扫描基准二进制。
- CI 中 1 MiB 完整扫描烟测。
- Linux 部署和性能基准执行文档。
- 独立 stdio MCP 协议烟测。
- 独立 Streamable HTTP MCP 协议烟测。
- Streamable HTTP 会话、SSE、分页、上下文和会话删除验证。
- 部署 doctor 烟测。
- 并发扫描基准入口。

当前 CI 基线：

```text
rustfmt: passed
Clippy -D warnings: passed
Rust tests: 90 passed
stdio MCP smoke: passed
Streamable HTTP MCP smoke: passed
deployment doctor smoke: passed
benchmark smoke: passed
```

协议烟测验证了：

- 协议版本协商为 `2025-06-18`。
- 三个 MCP 工具均可发现和调用。
- stdio 与 Streamable HTTP 均能查询真实临时日志。
- Streamable HTTP 服务端分配并接受 `Mcp-Session-Id`。
- 后续请求携带 `MCP-Protocol-Version`。
- SSE 响应能够正确解析，包括空 keepalive 事件。
- 搜索分页和 `match_ref` 上下文链路通过。
- HTTP DELETE 会话结束通过。
- SIGTERM 优雅停止通过。

性能烟测验证了：

- 生成器产生精确的 1 MiB 日志。
- 不存在关键字时扫描到文件末尾。
- 扫描字节数等于文件大小。
- 停止原因为 `Complete`。

这些烟测验证功能链路，不代表生产性能或所有 AI 客户端兼容性结论。

转为生产 GO 前仍需：

1. MCP Inspector 和至少一个实际 AI 客户端验证。
2. 1 GiB、10 GiB 和 10,000 小文件基准。
3. 1、4、8 并发 CPU、RSS、吞吐和 P99 数据。
4. 客户端断连取消端到端验证。
5. 日志持续追加、轮转、截断和删除压力测试。
6. systemd/cgroup 资源限制实测。
7. `RESOLVE_NO_XDEV` 部署决策。
8. 明确 `newest_first` 是否进入首期。
