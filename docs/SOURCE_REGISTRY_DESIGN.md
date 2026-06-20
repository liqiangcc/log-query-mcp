# SourceRegistry 与安全文件发现设计

## 1. 目标

`SourceRegistry` 把管理员配置转换为运行时可用的日志来源，同时确保 MCP 客户端永远不能通过来源选择绕过服务器文件边界。

## 2. 数据流

```text
AppConfig
   ↓ 结构校验
启用的 LogSourceConfig
   ↓
SafeRoot::open(root)
   ↓
显式文件安全验证 + 目录规则发现
   ↓
ConfiguredLogSource
   ↓
公开 LogSourceDescriptor + 不透明 file_id
```

公开描述中不包含来源 root、显式文件列表、目录规则或服务器绝对路径。

## 3. 安全打开

所有来源内文件和目录均从持有的根目录文件描述符相对打开，使用：

```text
openat2()
RESOLVE_BENEATH
RESOLVE_NO_SYMLINKS
RESOLVE_NO_MAGICLINKS
RESOLVE_NO_XDEV
```

打开后通过 `fstat` 确认：

- 文件读取只接受普通文件。
- 目录发现只接受目录。

文件在启动验证后发生变化时，后续实际打开仍会重新执行相同安全校验。

## 4. 目录发现

目录发现支持：

- 管理员配置的相对目录。
- 区分大小写的文件名后缀白名单。
- 可选递归。
- 目录项稳定排序。
- 重叠规则去重。

目录遍历不跟随软链接，并限制：

```text
规则数量
访问目录数量
读取目录项数量
匹配文件数量
```

`d_type` 为未知时，不根据路径字符串猜测类型，而是重新通过 `SafeRoot` 尝试安全打开和分类。

## 5. 文件标识

每个来源加载后的文件获得来源内不透明 `file_id`：

```text
file-<source-index>-<file-index>
```

`file_id` 不是服务器路径，客户端不能通过它选择未注册文件。真正打开时，服务端从注册表找到相对路径并重新经过 `SafeRoot`。

## 6. 日志轮转语义

当前切片在构建 `SourceRegistry` 时形成初始文件集合。多文件查询编排阶段将增加查询时目录快照刷新，用于发现服务启动后新产生的轮转文件。

即使文件集合刷新，所有新候选仍必须经过：

```text
目录规则
→ openat2()
→ 普通文件校验
→ 查询文件数上限
```

## 7. 错误边界

内部错误区分来源 root、显式文件、目录发现和文件打开问题；MCP 层后续统一映射为去敏错误代码：

```text
UNKNOWN_SOURCE
SOURCE_UNAVAILABLE
RESOURCE_LIMIT
```

不得把绝对路径或底层系统调用文本直接返回给客户端。
