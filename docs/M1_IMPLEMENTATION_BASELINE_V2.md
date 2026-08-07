# Log Query MCP v2 M1 实现基线

> 状态：M1 implementation complete  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`  
> 上游方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> 实施清单：[`REMOTE_SSH_CACHE_TODO_V2.md`](./REMOTE_SSH_CACHE_TODO_V2.md)

## 1. M1 目标

M1 只完成两件事：

1. 建立严格的 v1 / v2 配置版本路由和 v2 类型模型。
2. 把现有 Local Source 文件访问从 `SourceRegistry` 中抽离到 Backend 层。

M1 **不实现 SSH/SFTP Transport、远程同步和 CacheStore**。

因此有效的 v2 SSH 配置可以被解析和校验，但运行时必须明确返回 Backend 尚未实现，而不是静默退化、远程执行 Shell 或伪装成 Local Source。

---

## 2. 已实现结构

### 2.1 配置版本路由

新增：

```text
ConfigDocument
├── V1(AppConfig)
└── V2(AppConfigV2)
```

加载流程：

```text
读取 JSON
  ↓
只探测 version
  ↓
version=1 → 严格 v1 parser
version=2 → 严格 v2 parser
其他版本 → UnsupportedVersion
```

v1 原有 `deny_unknown_fields`、来源校验和安全语义保持不变。

### 2.2 v2 配置模型

新增类型覆盖：

- SSH connections。
- Password / Private Key 认证配置。
- `known_hosts` Host Key 配置。
- Local / SSH backend。
- Remote Sync policy。
- `full` / `tail` / `from_now` bootstrap。
- Cache 配置。
- v2 Remote limits。

Password 配置只接受 `secret_ref`，不会接受普通 `password` 字段。

### 2.3 v2 Limits

新增 `LimitsConfigV2`，保留现有查询限制，并增加：

```text
max_concurrent_ssh_connections
max_sync_bytes_per_query
max_remote_files_per_source
```

Local 查询部分通过 `local_limits()` 投影到现有 `LimitsConfig`，因此 M1 不修改现有 Query / Context service 的执行模型。

### 2.4 Source Backend

当前内部结构：

```text
SourceRegistry
      ↓
ConfiguredSource
      ↓
SourceBackend
      ↓
LocalBackend
      ↓
SafeRoot / openat2
```

`SourceBackend` 当前只暴露查询真正需要的能力：

```text
snapshot_files
open_snapshot_file
open_configured_file
path_is_configured
```

没有设计成通用可写 `FileSystem` API。

### 2.5 LocalBackend 安全边界

`LocalBackend` 继续复用：

```text
SafeRoot
openat2
RESOLVE_BENEATH
RESOLVE_NO_SYMLINKS
RESOLVE_NO_MAGICLINKS
RESOLVE_NO_XDEV
```

M1 不降低 ADR-0003 / ADR-0006 的 v1 文件安全边界。

---

## 3. Runtime 行为

### v1

```text
version=1
→ ConfigDocument::V1
→ SourceRegistry
→ LocalBackend
→ existing Query Engine
```

行为保持兼容。

### v2 Local

```text
version=2
backend=local
→ AppConfigV2 validation
→ LocalBackend
→ existing Query Engine
```

可以正常启动和查询。

### v2 SSH

```text
version=2
backend=ssh
→ AppConfigV2 validation succeeds
→ SourceRegistry detects unavailable backend
→ BackendUnavailable
```

这是 M1 的预期行为。

SSH 真正接入属于 M2/M5，不允许在 M1 使用临时 Shell/grep 实现绕过架构边界。

---

## 4. 错误安全

新增的 v2 配置错误和 `BackendUnavailable` 不直接暴露内部配置、主机、账号或路径信息给 MCP 客户端。

在现有 Tool Error 映射中，它们被视为服务内部配置/能力状态，客户端继续得到去敏后的稳定错误模型。

---

## 5. 测试

新增测试覆盖：

- v1 / v2 配置版本路由。
- 未知版本拒绝。
- v2 Local contract fixture 解析。
- v2 SSH Password contract fixture 解析。
- 未知 SSH connection 引用拒绝。
- v2 Remote limits 解析。
- v2 Local Source 可以通过 LocalBackend 建立 Registry 和文件 Snapshot。
- v2 SSH Source 在 M1 明确返回 `BackendUnavailable`。
- v1 原有 SourceRegistry / SafeRoot / Query 测试保持通过。

## 6. CI Gate

2026-08-07 M1 代码基线已通过：

```text
cargo fmt --all -- --check                         PASS
cargo clippy --locked --all-targets --all-features -- -D warnings  PASS
cargo test --locked --all-targets --all-features  PASS
cargo build --release --locked --bins              PASS
Contracts v1 + v2                                  PASS
```

验证运行：

- Rust workflow run: `31162992856`
- Contracts workflow run: `31162992948`

---

## 7. 已发现的契约偏差

M1 核对过程中发现：

```text
schemas/log-query-mcp-config-v2.schema.json
```

当前把：

```text
max_context_lines_per_side
```

定义为：

```text
0..1000
```

而现有 v1 Local Query / Context 配置硬边界仍是：

```text
1..50
```

M1 不通过放宽 v1 安全边界来掩盖这个差异。

在正式合并 v2 配置契约前必须二选一并冻结：

1. 把 v2 Schema 修正为当前实际支持的 `1..50`；或
2. 单独设计并验证 v2 Context 扩容语义，再提高运行时硬上限。

在该决策完成前，不应宣称 v2 Schema 与运行时在该字段上完全等价。

这不阻塞 M2 的 SSH/SFTP Transport 技术实现，因为 M2 不依赖 Context line 上限，但必须在 v2 发布前解决。

---

## 8. M1 Gate 结论

M1 的代码目标已经满足：

```text
v1 / v2 配置路由完成
Local Source 已通过 Backend 抽象
v2 Local 可运行
v2 SSH 配置可校验但不会被错误启用
v1 文件安全边界未回退
Rust 全量 CI 通过
Contracts CI 通过
```

下一阶段进入：

```text
M2：SSH/SFTP Transport
```

M2 应继续遵守 ADR-0008 / ADR-0011：只实现受控只读 SFTP 能力，不实现 Remote Exec。
