# Log Query MCP

面向 AI 问题排查的只读日志搜索 MCP 服务。

Log Query MCP 部署在能够访问日志文件的 Linux 服务器上。管理员配置允许查询的日志来源，本地 AI 通过 MCP 搜索运行日志，并结合本地代码仓库定位开发环境或测试环境问题。

> 项目处于正式实现阶段。v1 契约已经冻结，当前已完成版本化配置、安全来源注册表、有界流式扫描器和阻塞执行器，尚未形成生产发布版本。

## 工作方式

```text
用户描述问题
      ↓
本地 AI 读取代码仓库
      ↓
AI 调用 Log Query MCP
      ↓
MCP 搜索白名单日志文件
      ↓
AI 结合代码与日志诊断问题
```

职责划分：

- **本地 AI**：读取代码、分析日志、诊断问题。
- **Log Query MCP**：安全搜索服务器日志，返回匹配位置和有限上下文。
- **管理员**：配置允许查询的日志来源和只读权限。

## v1 工具

```text
list_log_sources
search_logs
get_log_context
```

核心能力：

- 按字面量关键字搜索一个或多个来源。
- 支持 `requestId`、`traceId`、异常名、错误码和业务 ID。
- 支持 RFC 3339 时间范围、分页和有限上下文。
- 限制扫描字节、结果数量、响应大小、上下文和并发任务。
- 不接受客户端服务器路径，不执行 Shell，不修改日志。

## v1 已冻结决策

- 目标平台：Linux kernel `>= 5.6`。
- 正式传输：Streamable HTTP；stdio 用于本机调试。
- 文件访问：来源白名单 + `openat2()` + `RESOLVE_NO_XDEV` + 普通文件校验。
- 搜索：字面量子串；不区分大小写仅保证 ASCII。
- 时间区间：`[start_time, end_time)`。
- 排序：v1 只支持 `oldest_first`。
- `match_ref` 和 cursor：单实例、服务端有状态、短期随机 token，重启后失效。
- 错误：稳定错误代码 + 去敏消息 + `retryable`。
- 内网使用：v1 不内置认证和 TLS。

## 正式化文档

- [v1 实现基线](./docs/IMPLEMENTATION_BASELINE_V1.md)
- [v1 MCP API](./docs/MCP_API_V1.md)
- [v1 错误模型](./docs/ERROR_MODEL_V1.md)
- [v1 配置 Schema 说明](./docs/CONFIG_SCHEMA_V1.md)
- [架构决策记录](./docs/adr/README.md)
- [MCP 工具机器 Schema](./schemas/mcp-tools-v1.schema.json)
- [工具错误机器 Schema](./schemas/tool-error-v1.schema.json)
- [服务配置机器 Schema](./schemas/log-query-mcp-config-v1.schema.json)
- [完整需求文档](./REQUIREMENTS.md)

## 当前实现进度

- [x] M1 v1 契约冻结
- [x] Rust 正式工程和版本化配置模型
- [x] `SafeRoot` 与 Linux `openat2()` 文件边界
- [x] 有界目录发现和 `SourceRegistry`
- [x] device/inode/size 文件快照
- [x] 有界流式字面量扫描器
- [x] 绝对行号、字节偏移和安全续扫位置
- [x] `spawn_blocking`、Semaphore、deadline 和协作取消
- [x] 安全文件到扫描结果的真实集成测试
- [ ] 多文件和多来源查询编排
- [ ] 时间过滤和稳定排序
- [ ] MCP Server
- [ ] cursor、`match_ref` 和上下文读取
- [ ] 目标 Linux 环境验收

详细状态见 [M2_STATUS.md](./docs/M2_STATUS.md)。

## 首期不包含

- `newest_first` 实际扫描。
- 正则表达式或复杂查询语言。
- 压缩日志和实时 tail。
- Kubernetes、Loki、Elasticsearch。
- 多实例共享 cursor / `match_ref`。
- 自动根因分析和代码修复。

## License

暂未指定。
