# Log Query MCP v2 M7 ProxyCommand Key Authentication Gate

> 状态：Harness implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25

## 1. 目标

验证 ProxyCommand 只负责底层 SSH raw byte stream，不改变 SSH Authentication 语义。

独立覆盖：

```text
unencrypted private key
encrypted private key + passphrase SecretResolver
```

两条路径都必须经过：

```text
ProxyCommand
→ SSH handshake
→ strict known_hosts
→ public-key authentication
→ SFTP
→ read-only stat/read_range
```

## 2. 独立 Gate

```text
tests/m7_proxy_auth_live.rs
.github/workflows/m7-proxy-auth.yml
```

CI fixture 创建：

- 一把无口令 Ed25519 client key；
- 一把带 passphrase 的 Ed25519 client key；
- 两把 public key 都写入同一 `authorized_keys`；
- `PasswordAuthentication no`；
- `PubkeyAuthentication yes`；
- ProxyCommand 使用 `/usr/bin/nc {host} {port}`。

账号显式设置一个未使用的密码，仅用于解除 Linux locked-account 状态；sshd 仍禁止 password authentication。

## 3. 无口令私钥

配置：

```json
{
  "auth": {
    "type": "private_key",
    "key_file": "<admin configured local key>"
  }
}
```

不配置 `passphrase_secret_ref`。

预期：

```text
ProxyCommand stream established
→ host key verified
→ private key auth succeeds
→ SFTP stat/read_range succeeds
```

## 4. 加密私钥

配置：

```json
{
  "auth": {
    "type": "private_key",
    "key_file": "<admin configured encrypted key>",
    "passphrase_secret_ref": "M7_AUTH_KEY_PASSPHRASE"
  }
}
```

passphrase 仍由现有 SecretResolver 解析，不进入 ProxyCommand argv/stdin/stderr，也不新增 Proxy credential channel。

## 5. 安全边界

ProxyCommand 不接收：

```text
username
password
private-key bytes
passphrase
secret_ref
remote path
```

认证仍完全属于 SSH 层。

MCP API 不新增任何 command/auth 参数。

## 6. 当前执行状态

候选 `9abb48c20801ffb0fce63ada609716652f37d88d` 已触发：

```text
workflow: M7 Proxy Auth
run:      31378855432
job:      proxy-auth-live
result:   failure
steps:    null
```

runner 未执行任何 step，与 Issue #23 的 GitHub Actions Billing / Spending Limit blocker 一致。

因此当前只能记录：

```text
unencrypted-key harness  IMPLEMENTED
encrypted-key harness    IMPLEMENTED
workflow recognition     CONFIRMED
actual execution         BLOCKED
PASS evidence            NONE
```

## 7. 完成条件

- [ ] unencrypted private-key path PASS。
- [ ] encrypted private-key + passphrase path PASS。
- [ ] strict known_hosts PASS。
- [ ] SFTP read-only operations PASS。
- [ ] secret/passphrase 不进入 ProxyCommand diagnostics/argv PASS。
- [ ] rustfmt / clippy / all-targets regression PASS。

真实 gate 通过前，本项不能标记 production evidence complete。
