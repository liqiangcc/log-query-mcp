# Linux 部署指南

> 当前状态：技术预研版本  
> 目标平台：Linux kernel 5.6 及以上

## 1. 部署前提

- 服务仅部署在受控内网。
- 目标服务器能够直接读取所配置的日志文件。
- Linux kernel 必须支持 `openat2()`，建议 5.6 及以上。
- MCP 服务使用独立的非 root 用户运行。
- 配置文件和日志文件对运行用户只读。
- 首期不提供身份认证和 TLS，监听地址由内网、防火墙或反向代理控制。

确认内核版本：

```bash
uname -r
```

## 2. 构建

在构建机执行：

```bash
git clone <repository-url>
cd log-query-mcp
git switch spike/technical-research
cargo build --release --locked
```

输出文件：

```text
target/release/log-query-mcp
```

预研阶段必须提交 `Cargo.lock`，正式发布应从固定提交构建。

## 3. 创建运行用户

```bash
sudo useradd \
  --system \
  --home-dir /nonexistent \
  --shell /usr/sbin/nologin \
  log-query-mcp
```

运行用户只需要：

- 读取 `/etc/log-query-mcp/config.json`。
- 遍历已配置日志目录。
- 读取明确配置或由管理员目录规则发现的日志文件。
- 监听配置的内网 TCP 地址和端口。

不要授予：

- root 权限。
- sudo 权限。
- 日志目录写权限。
- 配置目录写权限。

## 4. 安装二进制和配置

```bash
sudo install -o root -g root -m 0755 \
  target/release/log-query-mcp \
  /usr/local/bin/log-query-mcp

sudo install -d -o root -g log-query-mcp -m 0750 \
  /etc/log-query-mcp

sudo install -o root -g log-query-mcp -m 0640 \
  deploy/log-query-mcp.example.json \
  /etc/log-query-mcp/config.json
```

根据目标服务器修改：

```text
/etc/log-query-mcp/config.json
```

## 5. 日志来源配置

一个来源可以同时配置显式文件和目录发现规则。

```json
{
  "sources": [
    {
      "source_id": "payment-test",
      "name": "支付服务测试环境",
      "description": "payment-service application logs",
      "service": "payment-service",
      "environment": "test",
      "tags": ["payment", "java"],
      "root": "/var/log/payment-service",
      "files": ["application.log"],
      "directories": [
        {
          "path": "archive",
          "recursive": true,
          "include_suffixes": [".log", ".log.1"]
        }
      ],
      "timestamp_rule": {
        "type": "rfc3339",
        "prefix_bytes": 64
      }
    }
  ]
}
```

规则：

- `root` 是管理员可信配置，不通过 MCP 返回。
- `files` 必须是相对于 `root` 的规范化路径。
- `directories[].path` 使用 `.` 表示来源根目录，或使用规范化相对目录。
- `recursive=false` 只查看当前目录。
- `include_suffixes` 按文件名大小写敏感后缀匹配。
- 客户端不能提交路径、目录规则或后缀规则。
- 目录发现不跟随软链接，并受目录数、条目数和文件数硬限制。
- 显式文件和发现结果会去重并按稳定路径顺序保存。

禁止在路径中使用：

```text
绝对路径
.
..
软链接逃逸
```

当前目录规则使用后缀匹配，尚未提供通用 glob 表达式。

## 6. 授予日志只读权限

优先使用现有日志组：

```bash
sudo usermod -aG application-logs log-query-mcp
```

或使用 ACL 只授予读取和目录遍历权限：

```bash
sudo setfacl -m u:log-query-mcp:rx /var/log/payment-service
sudo setfacl -m u:log-query-mcp:r /var/log/payment-service/application.log
```

目录发现需要运行账号对配置目录链路拥有遍历权限。轮转工具创建新文件时，也需要保证新日志继承适当的组或 ACL。

验证运行用户能够读取，但不能写入：

```bash
sudo -u log-query-mcp test -r /var/log/payment-service/application.log
sudo -u log-query-mcp test ! -w /var/log/payment-service/application.log
```

## 7. 安装 systemd 服务

```bash
sudo install -o root -g root -m 0644 \
  deploy/log-query-mcp.service \
  /etc/systemd/system/log-query-mcp.service

sudo systemd-analyze verify \
  /etc/systemd/system/log-query-mcp.service

sudo systemctl daemon-reload
sudo systemctl enable --now log-query-mcp
```

查看状态：

```bash
systemctl status log-query-mcp
journalctl -u log-query-mcp -f
```

服务支持 `SIGTERM` 优雅停止：

```bash
sudo systemctl stop log-query-mcp
```

## 8. 基础运行参数

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `LOG_QUERY_MCP_BIND` | `127.0.0.1:8000` | Streamable HTTP 监听地址 |
| `LOG_QUERY_MCP_CONFIG` | `log-query-mcp.json` | JSON 配置文件路径 |
| `RUST_LOG` | `log_query_mcp=info,tower_http=info` | 日志级别过滤 |

建议默认只监听回环地址，再由受控内网代理转发。需要直接监听内网地址时：

```ini
Environment=LOG_QUERY_MCP_BIND=10.0.0.20:8000
```

不要监听公网地址。

## 9. 查询资源限制

所有覆盖值都必须是正的十进制整数。服务在启动时校验，非法、为零、超过硬上限或相互冲突的配置会导致启动失败。

| 环境变量 | 默认值 | 硬上限 | 说明 |
|---|---:|---:|---|
| `LOG_QUERY_MCP_MAX_SCAN_BYTES_PER_PAGE` | 536870912 | 68719476736 | 单页最大扫描字节数 |
| `LOG_QUERY_MCP_MAX_RETURNED_CONTENT_BYTES` | 524288 | 16777216 | 搜索结果日志正文合计 |
| `LOG_QUERY_MCP_MAX_RESPONSE_BYTES` | 1048576 | 67108864 | 完整 MCP JSON 响应上限 |
| `LOG_QUERY_MCP_QUERY_TIMEOUT_MILLIS` | 10000 | 600000 | 查询总 deadline，包含排队和扫描 |
| `LOG_QUERY_MCP_MAX_CONCURRENT_SCANS` | 4 | 64 | 同时执行的阻塞扫描任务数 |
| `LOG_QUERY_MCP_MATCH_REFERENCE_CAPACITY` | 10000 | 1000000 | 活动 `match_ref` 数量 |
| `LOG_QUERY_MCP_MATCH_REFERENCE_TTL_SECONDS` | 600 | 86400 | `match_ref` 有效期 |
| `LOG_QUERY_MCP_CURSOR_CAPACITY` | 1000 | 1000000 | 活动分页游标数量 |
| `LOG_QUERY_MCP_CURSOR_TTL_SECONDS` | 300 | 86400 | 分页游标有效期 |
| `LOG_QUERY_MCP_CONTEXT_MAX_BACKTRACK_BYTES` | 4194304 | 67108864 | 上下文向前回溯字节数 |
| `LOG_QUERY_MCP_CONTEXT_MAX_FORWARD_BYTES` | 4194304 | 67108864 | 上下文向后读取字节数 |
| `LOG_QUERY_MCP_CONTEXT_MAX_LINE_BYTES` | 16384 | 1048576 | 单条上下文日志最大正文 |
| `LOG_QUERY_MCP_CONTEXT_MAX_RETURNED_CONTENT_BYTES` | 524288 | 16777216 | 上下文正文合计 |
| `LOG_QUERY_MCP_CONTEXT_READ_BUFFER_BYTES` | 65536 | 1048576 | 上下文读取缓冲区 |

额外关系约束：

```text
搜索正文上限 < 完整响应上限
上下文单行上限 <= 上下文正文合计
上下文正文合计 < 完整响应上限
```

systemd 覆盖示例：

```ini
[Service]
Environment=LOG_QUERY_MCP_QUERY_TIMEOUT_MILLIS=15000
Environment=LOG_QUERY_MCP_MAX_CONCURRENT_SCANS=8
Environment=LOG_QUERY_MCP_MAX_SCAN_BYTES_PER_PAGE=1073741824
Environment=LOG_QUERY_MCP_MAX_RESPONSE_BYTES=2097152
```

修改后执行：

```bash
sudo systemctl daemon-reload
sudo systemctl restart log-query-mcp
```

启动日志会记录实际采用的扫描字节、响应大小、查询超时和并发数，但不会记录服务器日志路径或完整搜索关键字。

## 10. 客户端验证

启动服务后，MCP 地址为：

```text
http://127.0.0.1:8000/mcp
```

使用 MCP Inspector：

```bash
npx @modelcontextprotocol/inspector
```

在 Inspector 中选择 Streamable HTTP，并填写服务地址。

至少验证：

1. 能发现三个工具。
2. `list_log_sources` 不返回绝对路径。
3. `search_logs` 能查询一个真实日志文件。
4. 无匹配返回空数组。
5. 未知 `source_id` 返回受控错误。
6. `get_log_context` 能使用真实 `match_ref` 返回有限上下文。
7. 结果过多时能够使用 `next_cursor` 继续查询。
8. 服务日志中不记录完整业务关键字。

## 11. systemd 加固说明

示例 unit 使用：

- `NoNewPrivileges=true`
- `ProtectSystem=strict`
- `ProtectHome=true`
- `PrivateDevices=true`
- 空能力集合
- 文件描述符、任务数、内存和 CPU 限制

这些值是预研默认值。正式部署前应运行：

```bash
systemd-analyze security log-query-mcp.service
```

若日志位于特殊挂载点或服务依赖额外地址族，应按最小权限原则调整，而不是整体关闭加固。

## 12. 更新和回滚

更新前：

```bash
sudo cp /usr/local/bin/log-query-mcp \
  /usr/local/bin/log-query-mcp.previous
```

替换二进制并重启：

```bash
sudo install -o root -g root -m 0755 \
  target/release/log-query-mcp \
  /usr/local/bin/log-query-mcp
sudo systemctl restart log-query-mcp
```

回滚：

```bash
sudo mv /usr/local/bin/log-query-mcp.previous \
  /usr/local/bin/log-query-mcp
sudo systemctl restart log-query-mcp
```

服务重启后，内存中的 `match_ref` 和分页游标会失效，客户端需要重新搜索。

## 13. 当前部署边界

预研版本仍有以下限制：

- 暂无独立健康检查端点。
- 暂无热加载配置。
- 暂无认证和 TLS。
- 目录发现只支持后缀规则，尚未支持通用 glob。
- `newest_first` 尚未接入真实前向扫描链路。
- 多实例部署时，引用和游标状态不共享。
- `RESOLVE_NO_XDEV` 是否启用需根据目标挂载结构决定。

这些限制不影响单实例、受控内网中的首轮技术验证，但正式上线前必须根据验收范围逐项确认。
