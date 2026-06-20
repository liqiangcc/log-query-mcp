# ADR-0003：使用 openat2 建立文件访问边界

- 状态：Accepted
- 日期：2026-06-20

## 决策

1. 启动时打开并持有每个来源根目录的文件描述符。
2. 文件始终按相对路径解析。
3. 使用 Linux `openat2()`。
4. 启用 `RESOLVE_BENEATH`、`RESOLVE_NO_SYMLINKS`、`RESOLVE_NO_MAGICLINKS` 和 v1 的 `RESOLVE_NO_XDEV`。
5. 打开后使用 `fstat`，只接受普通文件。
6. 客户端不能提交服务器路径。

## 原因

内核在实际打开时执行路径约束，能够比字符串前缀、一次性 `realpath` 或只设置最终 `O_NOFOLLOW` 更可靠地阻止目录逃逸和竞态替换。

## 后果

- 要求 Linux kernel 5.6 及以上。
- 日志来源根内的嵌套挂载点需要拆成独立来源。
- 硬链接威胁仍依赖目录只读和写权限控制。
- 旧内核不得静默降级。
