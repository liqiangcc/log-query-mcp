# Architecture Decision Records

| ADR | 决策 | 状态 |
|---|---|---|
| [0001](./0001-use-rust-and-rmcp.md) | 使用 Rust 和官方 rmcp SDK | Accepted |
| [0002](./0002-use-streamable-http.md) | 远程服务使用 Streamable HTTP | Accepted |
| [0003](./0003-safe-file-access-with-openat2.md) | 使用 openat2 建立文件访问边界 | Accepted |
| [0004](./0004-bounded-blocking-scanner.md) | 使用有界同步扫描器和受限阻塞执行器 | Accepted |
| [0005](./0005-stateful-match-references-and-cursors.md) | match_ref 和 cursor 使用服务端有状态随机 token | Accepted for single-instance v1 |
| [0006](./0006-enable-resolve-no-xdev-in-v1.md) | v1 启用 RESOLVE_NO_XDEV | Accepted for v1 |
| [0007](./0007-support-remote-sources-via-local-cache.md) | Remote Source 通过本地 Cache 接入统一查询引擎 | Proposed for v2 |
| [0008](./0008-use-ssh-sftp-without-remote-exec.md) | Remote Source 仅使用 SSH/SFTP，不提供远程命令执行 | Proposed for v2 |
| [0009](./0009-use-cache-generations-and-query-snapshots.md) | Remote Cache 使用 Generation 与查询快照保证一致性 | Proposed for v2 |
| [0010](./0010-use-on-query-sync-and-explicit-cache-coverage.md) | v2 默认按查询同步并显式表达缓存覆盖范围 | Proposed for v2 |

推翻已有决策时应新增 ADR，并标记旧 ADR 为 Superseded，不应静默重写历史原因。
