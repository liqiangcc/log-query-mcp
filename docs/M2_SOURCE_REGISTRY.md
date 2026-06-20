# M2 安全来源注册表实现说明

> 分支：`feat/source-registry`  
> 状态：实现完成，待 PR 评审

## 1. 目标

把 v1 管理员配置转换为可供后续查询引擎使用的运行时来源，同时确保：

- 客户端不能改变服务器文件范围。
- 路径解析由 Linux 内核约束。
- 目录发现有固定资源边界。
- 查询开始时能够获得文件身份快照。
- 文件轮转、替换或截断不会把旧查询位置错误地应用到新文件。

## 2. 核心类型

### `SafeRoot`

启动时打开管理员配置的来源根目录，并长期持有目录 FD。后续文件和目录均相对于该 FD 解析。

### `SourceRegistry`

保存所有启用来源，并提供：

```text
list()
get(source_id)
selected(source_ids)
limits()
```

禁用来源保留在静态配置中，但不会打开 root，也不会出现在运行时来源列表中。

### `ConfiguredSource`

保存：

```text
公开来源描述
SafeRoot
显式文件路径
目录配置规则
编译后的发现规则
时间戳规则
```

提供候选文件快照和受控文件重新打开能力。

### `SourceFileSnapshot`

查询开始时记录：

```text
source_id
opaque file_id
relative_path
device
inode
size_at_snapshot
```

字段路径和身份信息只在服务端内部使用。

## 3. 安全文件打开

文件和目录使用：

```text
openat2(root_fd, relative_path)
```

解析标志：

```text
RESOLVE_BENEATH
RESOLVE_NO_SYMLINKS
RESOLVE_NO_MAGICLINKS
RESOLVE_NO_XDEV
```

文件打开标志包括只读、`CLOEXEC`、`NOFOLLOW` 和 `NONBLOCK`。打开后使用 `fstat` 确认对象类型。

因此 v1 会拒绝：

- 绝对路径。
- `.` 和 `..` 文件路径组件。
- 最终或中间软链接。
- procfs magic link。
- 来源 root 内的嵌套 mount 或 bind mount。
- 目录、FIFO、Socket 和设备文件作为日志文件。

来源 root 可以自身位于独立挂载点；禁止的是从 root FD 继续跨越到内部其他挂载点。

## 4. 目录发现

目录规则由管理员配置：

```json
{
  "path": "archive",
  "recursive": true,
  "include_suffixes": [".log", ".log.1"]
}
```

发现过程：

1. 通过 `SafeRoot` 安全打开规则目录。
2. 使用 FD 读取目录项。
3. 跳过 `.`、`..`、软链接和非 UTF-8 文件名。
4. 子目录只在 `recursive=true` 时进入，并再次通过 `openat2()` 打开。
5. 匹配后缀的普通文件再次通过 `open_regular_file()` 校验。
6. 结果按规范化相对路径排序并去重。

固定边界：

```text
单来源目录规则：最多 64
单规则后缀：最多 32
遍历目录项：最多 50,000
遍历目录：最多 10,000
单来源候选文件：最多 10,000
```

查询引擎后续还会应用更小的 `max_scan_files_per_query`。

## 5. 两层文件范围校验

文件能位于来源 root 下，不代表它一定被来源配置授权。

正式实现同时要求：

1. 路径通过 `openat2()` 的来源 root 边界。
2. 路径属于：
   - 显式文件列表；或
   - 某个目录规则的目录、递归和后缀范围。

`open_configured_file()` 会再次执行配置范围判断，因此后续 `match_ref` 或 cursor 的实现错误也不能轻易扩大到整个来源 root。

## 6. 文件快照

`snapshot_files(max_files)` 在查询开始时：

1. 重新安全打开全部显式文件。
2. 重新执行受控目录发现，以包含新产生的轮转文件。
3. 对显式和发现结果去重、排序。
4. 记录 device、inode 和当前大小。
5. 生成不包含路径明文的稳定 `file_id`。

`open_snapshot_file()` 重新打开文件时确认：

- 来源一致。
- 路径仍属于来源配置。
- device 和 inode 未变化。
- 当前大小不小于快照大小。

日志追加允许继续；替换或截断使快照失效。

## 7. 启动行为

`SourceRegistry::from_config()` 会：

- 再次执行配置结构校验。
- 跳过禁用来源。
- 打开每个启用来源 root。
- 编译目录规则。
- 验证显式文件。
- 执行一次受控发现，确认目录可访问且结果不突破绝对硬上限。

任何启用来源的关键安全验证失败都会阻止 Registry 构建。

## 8. 自动化测试

已覆盖：

- 普通文件安全打开和内容读取。
- root 与嵌套目录打开。
- 绝对路径、`..` 和 `./file` 拒绝。
- 最终和中间软链接拒绝。
- root 最终软链接拒绝。
- 目录、FIFO 和 Unix Socket 拒绝。
- 文件在打开前被替换为软链接。
- 解析标志包含 `RESOLVE_NO_XDEV`。
- 非递归后缀发现。
- 递归轮转文件发现。
- 软链接目录和文件不进入结果。
- 重叠规则去重。
- 发现文件数限制。
- 禁用来源不注册。
- 显式文件缺失导致启动失败。
- 显式与发现文件稳定合并。
- 未知来源选择失败。
- 文件替换后快照失效。
- 非递归规则不能授权嵌套路径。

## 9. 剩余验证

CI 环境没有 mount 权限，因此目前只验证代码确实启用了 `RESOLVE_NO_XDEV`。目标 Linux 验收必须增加：

1. 在来源 root 内创建嵌套 tmpfs 或 bind mount。
2. 确认显式文件打开被拒绝。
3. 确认目录发现不会进入该挂载点。
4. 确认错误映射为去敏后的 `SOURCE_UNAVAILABLE`。

## 10. 下一步

下一切片在 `SourceFileSnapshot` 和 `open_snapshot_file()` 之上实现：

```text
有界流式字面量扫描器
ScanExecutor
Semaphore 并发限制
CancellationToken
deadline
单文件扫描结果和停止原因
```
