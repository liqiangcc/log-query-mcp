# M2 cursor / match_ref 实现状态

当前切片已完成：

- 查询条件绑定。
- 固定候选文件快照。
- 全局排序水位线分页。
- cursor TTL、容量、租约和原子推进。
- 页面间累计资源使用。
- 每条返回结果的短期 `match_ref` 注册。
- 文件追加不进入既有 cursor 快照。
- 扫描字节限制中止时不生成误导性 cursor。
- Rustfmt、Clippy `-D warnings` 和全量 Rust 测试通过。

下一切片：

```text
有界 get_log_context
+ match_ref 文件身份复核
+ 前后行读取
+ 单行/内容/扫描字节限制
+ 文件轮转和篡改检测
```
