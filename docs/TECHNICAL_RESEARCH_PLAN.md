# 日志查询 MCP 技术预研计划

> 版本：v1.0  
> 日期：2026-06-19  
> 状态：阶段性完成，结论为 CONDITIONAL GO  
> 目标技术栈：Rust + 官方 `rmcp` SDK + Streamable HTTP

> 执行结果见 [TECHNICAL_RESEARCH_REPORT.md](./TECHNICAL_RESEARCH_REPORT.md)，正式实现安排见 [NEXT_PHASE_PLAN.md](./NEXT_PHASE_PLAN.md)。

---

## 1. 预研目的

本次预研不以完成正式产品为目标，而是验证当前需求能否基于 Rust 和官方 MCP Rust SDK 稳定实现，并为后续技术方案和正式开发提供可复现的决策依据。

预研结束后必须能够回答以下问题：

1. 官方 Rust MCP SDK 是否能够满足本项目三个工具的实现和客户端调用需求？
2. Streamable HTTP 是否能够在目标内网和实际 AI 客户端中稳定工作？
3. 日志文件扫描是否能够在限定内存、时间和并发条件下运行？
4. 是否能够可靠阻止路径穿越、软链接逃逸和特殊文件读取？
5. 请求取消、查询超时和资源上限是否能够真正终止扫描任务？
6. `match_ref` 和分页游标应采用怎样的首期设计？
7. 时间范围、日志轮转和上下文读取的实现边界是什么？
8. Rust 是否应被确定为正式实现语言，还是需要切换到 Go 等其他方案？

预研最终应形成明确结论：

```text
GO          使用 Rust 进入正式开发
CONDITIONAL 在补充指定约束后使用 Rust
NO-GO       放弃 Rust 或当前 SDK，改用其他实现方案
```

当前结论：

```text
CONDITIONAL GO
```

Rust 技术栈可以进入正式实现；实际 AI 客户端、目标服务器性能、断连取消和 systemd/cgroup 实测完成前，不建议宣布生产 GO。

---

## 2. 已确定的前提

### 2.1 产品前提

- 服务部署在能够读取日志文件的 Linux 服务器上。
- 服务仅在受控内网中使用。
- 首期不实现客户端认证、日志来源级授权和 TLS。
- MCP 只提供只读日志搜索，不执行 Shell 命令。
- AI 只能提交 `source_id`，不能提交服务器路径。
- 首期工具为：
  - `list_log_sources`
  - `search_logs`
  - `get_log_context`
- 首期只支持普通文本日志和未压缩轮转日志。
- 首期搜索采用字面量子串匹配，不支持正则表达式。

### 2.2 技术基线

预研默认采用：

| 类别 | 候选技术 |
|---|---|
| 语言 | Rust stable |
| MCP SDK | 官方 `rmcp` |
| 异步运行时 | Tokio |
| HTTP | Axum + Streamable HTTP |
| 序列化 | Serde / serde_json |
| JSON Schema | Schemars |
| 安全文件访问 | rustix + Linux `openat2()` |
| 日志与追踪 | tracing / tracing-subscriber |
| 错误处理 | thiserror |
| 基准测试 | 自定义集成基准和系统指标采集 |

官方 `rmcp` 已提供工具宏、输入/输出 Schema、stdio、Streamable HTTP Server 和 Tokio 异步运行时支持，并已通过本项目的独立协议烟测。

### 2.3 Linux 前提

安全文件打开采用 `openat2()`，因此目标环境为：

```text
Linux kernel >= 5.6
```

如果目标服务器存在更低版本内核，不得静默退化为仅依赖字符串前缀或一次性 `realpath` 校验的实现。

---

## 3. 预研范围

### 3.1 已验证

- Rust 工程和基础 CI。
- `rmcp` 工具定义、发现和调用。
- stdio 与 Streamable HTTP 协议烟测。
- 结构化输入和结构化输出。
- 普通字符串日志搜索。
- 显式文件日志来源和多日志来源查询。
- 有限上下文读取。
- 查询超时、主动取消和并发限制原语。
- 文件数、扫描字节数、结果数、单行和响应大小限制。
- `openat2()` 路径边界和软链接防护。
- 日志轮转、替换和截断场景。
- `match_ref` 和分页游标的最小可行方案。
- 时间范围、日志时间解析和稳定排序。
- systemd 部署脚手架、doctor 和性能烟测入口。

### 3.2 仍需目标环境验证

- 实际 AI 客户端兼容性。
- 1 GiB、10 GiB 和 10,000 小文件基准。
- 1、4、8 并发数据。
- 客户端断连后的取消延迟。
- 日志持续追加、轮转、截断和删除压力测试。
- systemd/cgroup 资源限制实测。
- `RESOLVE_NO_XDEV` 部署决策。

### 3.3 本次不做

- 用户认证和权限系统。
- TLS。
- Loki、Elasticsearch 或 Kubernetes 适配。
- 实时 tail/follow。
- 压缩日志查询。
- 正则表达式或复杂查询语言。
- 自动异常聚合和根因分析。
- Web 管理后台。
- 多节点分布式游标。
- 长期持久化索引。

---

## 4. 执行产出

主要代码：

```text
src/mcp_server.rs
src/query_engine.rs
src/runtime_config.rs
src/source_discovery.rs
src/safe_fs.rs
src/scanner.rs
src/scan_executor.rs
src/match_reference.rs
src/search_cursor.rs
src/context_reader.rs
src/time_filter.rs
```

主要文档：

```text
docs/TECHNICAL_RESEARCH_REPORT.md
docs/ARCHITECTURE.md
docs/adr/
docs/NEXT_PHASE_PLAN.md
docs/DEPLOYMENT.md
docs/PERFORMANCE_BENCHMARK.md
```

主要验证：

```text
rustfmt
Clippy -D warnings
Rust unit/integration tests
stdio MCP protocol smoke
Streamable HTTP MCP protocol smoke
deployment doctor smoke
benchmark smoke
```

---

## 5. 完成标准与当前状态

| 完成标准 | 状态 |
|---|---|
| 远程 MCP 客户端能调用三个工具 | 协议烟测通过；实际 AI 客户端待验证 |
| 能配置文件来源并搜索真实日志 | 通过 |
| 能通过 match_ref 获取上下文 | 通过 |
| 路径穿越和软链接测试不能越界 | 通过 |
| 查询能够超时或取消 | 原语和测试通过；断连端到端待验证 |
| 超长行和大结果受限制 | 通过 |
| 已确定语言、SDK、传输和扫描方案 | 通过 |
| 已形成预研报告和 ADR | 通过 |
| 目标环境完整性能数据 | 待完成 |
| systemd/cgroup 生产实测 | 待完成 |

---

## 6. 最终决策门槛

### 当前：CONDITIONAL GO

允许：

- 使用 Rust 进入正式实现。
- 冻结架构和接口。
- 在目标 Linux 环境开展验收。

暂不允许：

- 将 spike 分支直接视为生产版本。
- 在未完成性能和实际客户端验证前宣布上线。
- 在旧内核上静默降级文件安全方案。

### 转为生产 GO 的条件

详见 [TECHNICAL_RESEARCH_REPORT.md](./TECHNICAL_RESEARCH_REPORT.md) 第 12 节。
