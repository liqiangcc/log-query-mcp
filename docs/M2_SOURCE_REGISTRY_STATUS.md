# M2 SourceRegistry 与安全文件访问状态

当前切片实现：

```text
AppConfig
→ SourceRegistry
→ SafeRoot
→ 显式文件验证
→ 受控目录发现
```

安全基线：

- Linux `openat2()`。
- `RESOLVE_BENEATH`。
- `RESOLVE_NO_SYMLINKS`。
- `RESOLVE_NO_MAGICLINKS`。
- `RESOLVE_NO_XDEV`。
- 打开后只接受普通文件或目录。
- 目录发现有规则数、目录数、条目数和文件数上限。
- 显式文件和发现文件在每次实际打开时重新验证。

当前不包含 MCP Server、日志扫描器、cursor 或上下文读取。
