# 日志查询 MCP 架构设计草案

> 状态：技术预研结论草案  
> 适用范围：首期单实例 Linux 部署

---

## 1. 架构目标

系统需要在不开放任意服务器文件读取和命令执行的前提下，为 AI 提供适合问题排查的日志搜索能力。

核心质量目标：

1. 文件访问范围由管理员配置决定。
2. 大文件扫描使用有界内存和有界并发。
3. MCP 返回结构化、小批量、可继续查询的结果。
4. 匹配上下文按需展开，不提供通用文件浏览能力。
5. 日志轮转和文件变化不会导致越权读取或错误定位。
6. 协议层、业务编排、文件安全和扫描实现保持分离。

---

## 2. 系统上下文

```text
┌────────────────────┐
│ 用户               │
└─────────┬──────────┘
          │ 描述问题
┌─────────▼──────────┐
│ 本地 AI            │
│ - 读取代码仓库     │
│ - 调用 MCP         │
│ - 分析日志         │
└─────────┬──────────┘
          │ Streamable HTTP
┌─────────▼──────────┐
│ Log Query MCP      │
│ - 来源发现         │
│ - 字面量搜索       │
│ - 有限上下文       │
└─────────┬──────────┘
          │ 只读文件访问
┌─────────▼──────────┐
│ 服务器日志文件     │
└────────────────────┘
```

MCP 不读取本地代码仓库，也不判断根因。

---

## 3. 组件分层

### 3.1 MCP 传输层

职责：

- Streamable HTTP 和 stdio 传输。
- MCP 协议协商。
- 工具注册和 Schema 暴露。
- 将结构化请求交给应用服务。
- 将应用错误转换为工具错误。

不负责：

- 文件路径解析。
- 日志扫描。
- 游标和引用内部状态解释。

主要模块：

```text
src/main.rs
src/bin/log-query-mcp-stdio.rs
src/mcp_server.rs
src/model.rs
```

### 3.2 查询应用层

职责：

- 校验服务端资源上限。
- 解析查询时间范围。
- 选择日志来源。
- 创建候选文件快照。
- 编排单文件和多文件扫描。
- 生成 match_ref 和 cursor。
- 合并并排序跨来源结果。
- 限制完整 MCP 响应大小。

主要模块：

```text
src/query_engine.rs
src/limits_config.rs
```

### 3.3 配置与日志来源层

职责：

- 解析管理员 JSON 配置。
- 校验 source_id、根目录和相对文件列表。
- 打开并持有来源根目录文件描述符。
- 对 MCP 只暴露来源描述信息。
- 将来源 ID 映射到 SafeRoot 和文件集合。

主要模块：

```text
src/runtime_config.rs
src/source_discovery.rs
```

### 3.4 安全文件访问层

职责：

- 从已打开的根目录 FD 解析相对路径。
- 使用 `openat2()` 阻止目录逃逸和软链接。
- 打开后确认对象为普通文件。
- 返回文件身份和大小。

主要模块：

```text
src/safe_fs.rs
```

### 3.5 扫描层

职责：

- 对 `Read` 执行流式字面量匹配。
- 控制读取缓冲区和单行内存。
- 控制扫描字节、结果数量和返回内容大小。
- 检查取消和 deadline。
- 返回行号和字节偏移。

主要模块：

```text
src/scanner.rs
src/scan_executor.rs
src/cursor_reader.rs
```

### 3.6 短期状态层

职责：

- 管理不透明 match_ref。
- 管理不透明分页 cursor。
- TTL、容量和淘汰。
- 绑定文件身份、查询条件和扫描位置。

主要模块：

```text
src/match_reference.rs
src/search_cursor.rs
```

### 3.7 上下文和时间层

职责：

- 读取匹配位置前后的有限日志。
- 重新验证文件身份和匹配位置。
- 解析日志时间戳。
- 执行时间过滤和稳定排序。

主要模块：

```text
src/context_reader.rs
src/time_filter.rs
src/timestamp.rs
```

---

## 4. 搜索请求数据流

```text
search_logs request
        ↓
MCP Schema + runtime validation
        ↓
QueryService 解析查询条件
        ↓
SourceRegistry 查找来源
        ↓
生成候选文件身份快照
        ↓
创建或恢复 SearchCursorData
        ↓
SafeRoot 安全打开当前文件
        ↓
ScanExecutor 获取并发许可
        ↓
spawn_blocking 执行 scanner
        ↓
时间过滤、match_ref 注册、结果排序
        ↓
保存下一页 cursor
        ↓
检查完整 JSON 响应大小
        ↓
SearchLogsResponse
```

客户端请求中永远不包含服务器路径。

---

## 5. 上下文请求数据流

```text
get_log_context(match_ref)
        ↓
MatchReferenceStore 解析引用
        ↓
根据 source_id 获取 SourceRegistry 条目
        ↓
确认引用文件仍在该来源配置中
        ↓
SafeRoot 重新打开相对路径
        ↓
校验 device/inode、文件大小和匹配位置
        ↓
有界读取 before/after lines
        ↓
检查响应大小
        ↓
GetLogContextResponse
```

该流程不接受 `file_id + line_number` 任意组合。

---

## 6. 文件安全边界

### 6.1 信任边界

| 输入 | 信任级别 | 处理方式 |
|---|---|---|
| MCP `source_id` | 不可信 | 仅用于查找已配置来源 |
| MCP `keyword` | 不可信 | 只作为字面量字节序列 |
| MCP `cursor` / `match_ref` | 不可信 | 仅在有界服务端 Store 中查找 |
| 管理员配置 | 受控但需校验 | 启动时严格验证 |
| 日志文件内容 | 不可信 | 不解释为指令或协议 |
| 文件系统目录项 | 不可信 | openat2 + fstat |

### 6.2 安全打开

```text
configured root
    ↓ open O_PATH|O_DIRECTORY|O_NOFOLLOW
OwnedFd
    ↓ openat2(relative path)
RESOLVE_BENEATH
RESOLVE_NO_SYMLINKS
RESOLVE_NO_MAGICLINKS
    ↓ fstat
regular file only
```

旧内核不得静默退化成字符串前缀检查。

### 6.3 部署纵深防御

- 非 root 用户。
- 日志目录只读。
- 配置目录不可写。
- systemd `NoNewPrivileges=true`。
- systemd 文件系统保护。
- CPU、内存、FD 和任务数限制。
- 默认仅监听回环或指定内网地址。

---

## 7. 并发模型

```text
Tokio async runtime
├── MCP / HTTP 控制面
├── 会话与请求编排
└── Semaphore
      ↓
   spawn_blocking
      ↓
   同步文件扫描
```

关键规则：

1. 大文件读取不得在 Tokio 核心线程执行。
2. `spawn_blocking` 不是资源配额，必须额外使用 Semaphore。
3. 扫描许可在同步任务真正结束后释放。
4. 扫描循环协作检查取消和 deadline。
5. HTTP Future 被丢弃时应取消业务令牌。
6. 普通文件读取不可被强制中止，因此检查粒度决定取消延迟。

---

## 8. 状态模型

### 8.1 match_ref

```text
随机 token
→ source_id
→ relative_path
→ file identity
→ line / byte offsets
→ keyword semantics
→ expires_at
```

用途：有限上下文读取。

### 8.2 cursor

```text
随机 token
→ normalized query
→ candidate file snapshot
→ next file / byte / line
→ cumulative resource usage
→ expires_at
```

用途：继续获取下一批结果。

首期均为单实例内存状态，重启后失效。

---

## 9. 一致性模型

系统不锁定正在写入的日志文件，不提供查询快照事务。

采用尽力而为模型：

- 搜索开始时记录候选文件身份和大小。
- 文件追加可以继续发生。
- 文件被替换时，通过 device/inode 识别。
- 文件被截断到当前位置之前时，游标或引用失效。
- 相同 inode 被原地覆盖的所有变化无法仅靠 inode 检测；关键位置会额外复核。
- 跨服务时间排序依赖日志时钟，不代表严格因果顺序。

---

## 10. 时间模型

- 查询参数：RFC 3339。
- 区间：`[start, end)`。
- 来源可以配置日志行时间规则。
- 多行异常可继承事件首行时间。
- 无法解析或畸形时间的匹配保守返回。
- 文件名时间和 mtime 只用于候选排序。
- 无时间结果放在可解析时间结果之后。

---

## 11. 错误模型

对客户端保持少量稳定类别：

```text
invalid request
unknown source
query deadline exceeded
query cancelled
cursor/reference invalid or expired
configured file unavailable
resource or response limit reached
internal tool error
```

内部错误可以更细，但不得把以下内容返回给客户端：

- 服务器绝对路径。
- 系统调用参数。
- Rust backtrace。
- 配置文件位置。
- 服务器用户和凭证。

无匹配不是错误，返回空结果。

---

## 12. 可观测性

正式实现至少记录：

- 工具名称。
- source_id 集合。
- 查询耗时。
- 排队耗时和扫描耗时。
- 扫描文件数和字节数。
- 返回结果数和响应大小。
- 停止原因。
- cursor / match_ref 创建与失效数量。
- 取消和 deadline 次数。

默认不记录完整搜索关键字，可以记录长度或不可逆摘要。

---

## 13. 扩展点

后续能力应通过适配层扩展，而不是破坏现有安全边界：

- 目录 glob 文件发现。
- 压缩日志读取器。
- Loki / Elasticsearch 后端。
- Kubernetes 日志适配器。
- 实时 tail。
- 多实例共享 cursor/reference Store。
- 结构化 JSON 字段查询。

不同后端仍应实现统一的来源白名单、资源限制和结构化结果接口。

---

## 14. 首期部署拓扑

```text
受控内网
   │
   ▼
反向代理（可选）
   │
   ▼
单实例 log-query-mcp
   │ 非 root / systemd limits
   ▼
只读日志目录
```

首期不建议多实例负载均衡，因为 match_ref 和 cursor 不共享。若必须多实例，应启用会话粘性或引入共享状态。

---

## 15. 下一阶段架构工作

1. 冻结正式配置 Schema。
2. 冻结 MCP 工具输入输出 Schema。
3. 决定 `newest_first` 首期范围。
4. 将资源限制全部配置化并设置硬上限。
5. 增加结构化查询审计指标。
6. 完成目标服务器性能和取消测试。
7. 将预研模块迁移到正式实现分支并逐模块评审。
