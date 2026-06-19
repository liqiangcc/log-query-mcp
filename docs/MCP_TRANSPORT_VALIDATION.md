# MCP 传输兼容性验证

> 状态：独立协议烟测通过  
> 覆盖传输：stdio、Streamable HTTP

## 1. 验证目标

协议烟测不使用 MCP 客户端 SDK，而是直接构造 JSON-RPC 消息，用于验证服务端是否真正符合客户端所依赖的传输和工具调用契约。

验证范围包括：

- MCP 初始化和协议版本协商。
- `notifications/initialized` 生命周期通知。
- 工具发现及输入 Schema。
- 结构化工具输出。
- 真实日志来源查询。
- 搜索分页。
- `match_ref` 上下文读取。

## 2. stdio

脚本：

```text
research/scripts/mcp_stdio_smoke.py
```

执行：

```bash
cargo build --bin log-query-mcp-stdio
python3 research/scripts/mcp_stdio_smoke.py \
  --server target/debug/log-query-mcp-stdio
```

已验证：

- 每行一个 UTF-8 JSON-RPC 消息。
- stdout 不包含非协议文本。
- stderr 可独立记录服务日志。
- 初始化、工具发现、搜索、分页和上下文读取。

## 3. Streamable HTTP

脚本：

```text
research/scripts/mcp_http_smoke.py
```

执行：

```bash
cargo build --bin log-query-mcp
python3 research/scripts/mcp_http_smoke.py \
  --server target/debug/log-query-mcp
```

测试脚本会：

1. 创建临时真实日志和 JSON 配置。
2. 在随机本地端口启动 HTTP MCP Server。
3. 发送 `initialize` 请求。
4. 保存服务端返回的 `Mcp-Session-Id`。
5. 在后续请求中发送会话 ID 和 `MCP-Protocol-Version`。
6. 接受 `application/json` 或 `text/event-stream` 响应。
7. 发送 `notifications/initialized` 并验证 HTTP 202。
8. 调用三个 MCP 工具。
9. 验证搜索分页和上下文读取。
10. 使用 HTTP DELETE 结束会话。
11. 发送 SIGTERM 并验证服务优雅退出。

当前 `rmcp` 服务端在烟测中使用：

```text
Content-Type: text/event-stream
```

烟测客户端支持 SSE 注释、空数据事件和包含多个 `data:` 行的事件，不会把空 keepalive 事件误判为 JSON-RPC 消息。

## 4. 当前自动化结果

严格 CI 当前验证：

```text
rustfmt: passed
Clippy -D warnings: passed
Rust tests: 90 passed
stdio MCP smoke: passed
Streamable HTTP MCP smoke: passed
deployment doctor smoke: passed
benchmark smoke: passed
```

Streamable HTTP 报告确认：

- 协议版本：`2025-06-18`。
- 服务端分配了会话 ID。
- 响应使用 SSE。
- HTTP DELETE 返回 202。
- 三个工具均可发现和调用。
- 分页和上下文链路通过。
- SIGTERM 优雅退出通过。

## 5. 边界

该验证属于独立协议客户端兼容测试，不等同于所有 AI 产品的兼容认证。

仍需在目标环境记录：

- 实际 AI 客户端名称和版本。
- 客户端配置方式。
- Streamable HTTP 地址和网络访问策略。
- 工具 Schema 展示效果。
- 客户端取消请求的端到端行为。
- 长结果和错误结果的 UI 行为。

## 6. 结论

stdio 和 Streamable HTTP 的基础协议链路均已通过独立自动化验证。MCP SDK 不再是首期实现的主要阻塞风险；剩余兼容性工作主要是目标 AI 客户端和目标内网环境的实测。
