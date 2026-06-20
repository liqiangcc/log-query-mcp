# M2 查询状态：cursor 与 match_ref

> 分支：`feat/query-orchestration-time-filter`  
> 状态：实现中

## 1. 目标

在不向客户端暴露服务器路径、inode、候选文件列表和字节偏移的前提下，为完整扫描后的稳定结果提供分页，并为返回结果创建可用于后续上下文读取的短期 `match_ref`。

## 2. cursor 模型

客户端只获得：

```text
cur_<32 hex chars>
```

服务端内存保存：

```text
规范化查询条件
固定候选文件快照
上一页最后一条结果的全局排序水位线
累计页面、文件、字节、行、匹配和返回内容用量
TTL 和租约状态
```

查询条件绑定：

```text
source_ids（含顺序）
keyword
case_sensitive
start_time
end_time
max_results
```

任何条件变化均拒绝使用原 cursor。

## 3. 水位线分页

查询使用固定候选文件快照，并在每一页重新扫描该快照，只保留排序键严格大于上一页水位线的结果。

排序键：

```text
绝对时间（未知时间最后）
→ 来源请求顺序
→ 文件稳定顺序
→ 行号
→ 匹配字节偏移
```

该方法避免在 cursor 中缓存无界结果集合，并保持页面之间的确定性。代价是后续页会重新扫描固定快照，因此 cursor 同时累计扫描资源。

只有完整扫描了全部候选文件后，结果数或内容大小导致的截断才会生成下一页 cursor。若页面扫描字节限制导致候选未完全扫描，则返回 `truncated=true` 但不生成误导性的全局 cursor。

## 4. cursor 租约

续页请求先取得 cursor 租约：

- 同一 cursor 不能被两个请求并发推进。
- 查询条件不匹配时不消费 cursor。
- 执行失败或 Future 被取消时，Drop 自动释放租约，原 cursor 可重试。
- 成功完成后原 token 原子失效，并可生成新的 token。

## 5. 累计资源边界

cursor 累计：

```text
pages_returned
files_scanned
bytes_scanned
lines_scanned
raw_matches
eligible_matches
results_returned
returned_content_bytes
```

绝对硬上限：

```text
最多 100 页
累计扫描最多 64 GiB
累计返回结果最多 20,000 条
```

达到累计限制后仍可返回当前页结果，但不再生成下一页 cursor。

## 6. match_ref 模型

每条实际返回的结果注册：

```text
mref_<32 hex chars>
```

服务端保存：

```text
source_id
file_id
规范化相对路径
device / inode
搜索时文件大小
行号
行起点偏移
匹配偏移
原关键字和大小写语义
TTL
```

内部状态不实现 Serde 序列化，后续 `get_log_context` 必须通过 SourceRegistry 和 SafeRoot 重新验证文件身份和配置范围。

## 7. 一致性

- cursor 固定搜索开始时的候选文件和文件大小。
- 查询期间追加的新内容不进入该 cursor 的后续页。
- 文件替换、轮转或截断使查询失败，租约释放以便调用方决定重试或重新搜索。
- match_ref 服务重启、过期或被容量淘汰后失效。

## 8. 当前边界

- 本切片注册 `match_ref`，尚未实现 `get_log_context`。
- 尚未接入 MCP Server 和工具错误映射。
- 完整 MCP JSON 响应大小限制仍在后续接口层完成。
