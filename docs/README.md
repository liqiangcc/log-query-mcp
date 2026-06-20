# 文档索引

## 项目与结论

- [需求文档](../REQUIREMENTS.md)
- [技术预研计划](./TECHNICAL_RESEARCH_PLAN.md)
- [技术预研报告](./TECHNICAL_RESEARCH_REPORT.md)
- [架构设计草案](./ARCHITECTURE.md)
- [Architecture Decision Records](./adr/README.md)
- [正式实现阶段计划](./NEXT_PHASE_PLAN.md)

## MCP 与接口

- [工具 Schema 草案](./TOOL_SCHEMA_DRAFT.md)
- [MCP 传输验证](./MCP_TRANSPORT_VALIDATION.md)

## 核心技术预研

- [安全文件访问](./SAFE_FILE_ACCESS_RESEARCH.md)
- [流式扫描器](./SCANNER_RESEARCH.md)
- [阻塞扫描执行器](./EXECUTOR_RESEARCH.md)
- [match_ref](./MATCH_REFERENCE_RESEARCH.md)
- [上下文读取器](./CONTEXT_READER_RESEARCH.md)
- [分页游标](./SEARCH_CURSOR_RESEARCH.md)
- [时间范围与排序](./TIME_FILTER_RESEARCH.md)
- [时间戳解析补充](./TIMESTAMP_RESEARCH.md)

## 部署与验证

- [Linux 与 systemd 部署](./DEPLOYMENT.md)
- [性能基准执行指南](./PERFORMANCE_BENCHMARK.md)
- [R-10 状态](./R10_STATUS.md)

## 推荐阅读顺序

```text
REQUIREMENTS.md
→ TECHNICAL_RESEARCH_REPORT.md
→ ARCHITECTURE.md
→ adr/README.md
→ NEXT_PHASE_PLAN.md
→ TOOL_SCHEMA_DRAFT.md
→ DEPLOYMENT.md
```
