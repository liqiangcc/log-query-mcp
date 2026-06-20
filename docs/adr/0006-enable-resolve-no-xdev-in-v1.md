# ADR-0006：v1 安全文件访问启用 RESOLVE_NO_XDEV

- 状态：Accepted for v1
- 日期：2026-06-20

## 背景

`openat2()` 的 `RESOLVE_BENEATH`、`RESOLVE_NO_SYMLINKS` 和 `RESOLVE_NO_MAGICLINKS` 能限制目录逃逸和链接解析，但仍允许相对路径进入来源根目录下的其他挂载点或 bind mount。

日志部署中可能存在：

- 来源根目录本身位于独立日志磁盘。
- 来源根目录下面嵌套其他挂载点。
- bind mount 把其他目录映射到来源根目录内部。

v1 的目标是形成容易审计的最小文件边界，而不是自动穿越复杂挂载拓扑。

## 决策

v1 打开日志文件和发现目录时，除已有标志外启用：

```text
RESOLVE_NO_XDEV
```

完整解析约束为：

```text
RESOLVE_BENEATH
RESOLVE_NO_SYMLINKS
RESOLVE_NO_MAGICLINKS
RESOLVE_NO_XDEV
```

允许日志来源的 `root` 自身是一个独立挂载点；禁止从该 root 的文件描述符继续跨越到其内部嵌套的其他挂载点。

需要查询不同挂载点时，管理员应把每个挂载点配置为独立日志来源或独立来源根目录。

## 原因

- 缩小管理员误配置和 bind mount 引入的文件边界。
- 文件来源与一个明确文件系统根绑定，更易审计。
- 当前配置已支持多个来源，拆分挂载点不会要求客户端提交路径。
- 首期不需要自动跨越嵌套挂载点。

## 后果

正面：

- 阻止通过嵌套 mount 或 bind mount 读取来源根之外的文件系统对象。
- 安全测试和部署检查更明确。
- 日志来源边界与管理员配置一一对应。

负面：

- 一个来源根目录下存在合法嵌套挂载点时，这些文件不会被读取。
- 管理员可能需要把挂载点拆成多个来源。
- 某些容器和复杂 bind mount 部署需要调整配置。

## 实现要求

- `SafeRoot::open_regular_file` 和目录发现均使用 `RESOLVE_NO_XDEV`。
- deployment doctor 检查每个显式文件和目录规则是否会跨挂载点。
- 跨挂载失败对 MCP 客户端表现为 `SOURCE_UNAVAILABLE`，不返回绝对路径。
- 增加同文件系统成功和嵌套 mount 拒绝的 Linux 集成测试。

## 重新评估条件

只有目标环境证明必须在单个来源内跨越嵌套挂载点时，才新增配置级显式开关并通过新 ADR 评审。不得默认关闭该限制。
