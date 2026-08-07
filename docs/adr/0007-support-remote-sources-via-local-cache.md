# ADR-0007：Remote Source 通过本地缓存接入统一查询引擎

- 状态：Accepted for v2
- 日期：2026-08-07

## 决策

1. v2 在保留 Local Source 的基础上新增 Remote SSH Source。
2. Remote Source 不直接参与查询扫描；远程日志先通过受控同步进入本地 Cache，再由现有 Scanner / Query Engine 查询。
3. 查询引擎只面向本地稳定快照，不感知 SSH、SFTP、远程主机或凭证。
4. MCP 工具接口继续保持 `list_log_sources`、`search_logs`、`get_log_context`，不向客户端暴露同步和连接细节。
5. `source_id` 继续作为客户端选择日志来源的唯一入口；客户端不能提交服务器路径、主机地址或连接参数。
6. v1 配置和 Local Source 安全语义保持不变；Remote Source 使用 v2 配置契约。

## 原因

直接让 Query Engine 访问 SSH/SFTP 会把网络错误、远程文件变化和连接状态传播到扫描、分页和上下文逻辑中，显著增加复杂度。先同步到本地 Cache 可以复用现有扫描实现，并为 cursor、`match_ref` 和分页建立稳定快照。

该设计也使未来增加其他数据来源时保持清晰边界：Transport 负责获取数据，Cache 负责本地持久化与一致性，Query Engine 负责搜索。

## 后果

- Remote Source 查询前可能产生同步延迟。
- 本地磁盘会保存远程日志副本，因此需要严格权限、容量限制和 GC。
- 查询结果的完整性依赖缓存覆盖范围，缓存覆盖不完整时必须显式报错，不能返回假阴性的空结果。
- Local Source 继续使用 ADR-0003 的 `openat2()` 安全边界；Remote Source 使用独立的 SSH/SFTP 与服务器权限安全模型。
