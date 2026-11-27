# 文件监视记录字段

`file_monitor.log` 每行一条记录，主字段以 `|` 分隔，扩展字段以 `key=value` 追加。
带 `field_encoding=percent` 的记录里，字段内的 `%`、`|`、回车、换行分别编码为
`%25`、`%7C`、`%0D`、`%0A`。

## 主字段

| 位置 | 字段 | 说明 |
|---|---|---|
| 1 | 时间 | `YYYY-MM-DD HH:mm:ss`，设备本地时区。由记录生成方写入 |
| 2 | 进程 | 实际落盘进程的包名；shared-uid 组显示逗号连接的组标签 |
| 3 | 调用方 | 归因出的发起应用；未归因时为 `-` |
| 4 | 操作 | `OPEN`、`MKDIR`、`RENAME`、`UNLINK` 等 |
| 5 | 路径 | 请求路径（percent 编码） |

## 常用扩展字段

| 字段 | 取值 | 说明 |
|---|---|---|
| `ret` / `errno` | 整数 | `ret >= 0` 为成功；失败时为 `-1` 加 errno |
| `identify_method` | `caller`、`caller_uid`、`caller_pid`、`provider_open`、`saf_provider`、`shared_uid`、`executor`、`unknown` | 来源识别方式，见 [troubleshooting](troubleshooting.md#文件监视归因) |
| `identify_reliability` | `high`、`fallback`、`none` | 归因可信度 |
| `event_role` | `request`、`observation` | 请求记录，或文件系统观察记录 |
| `op` | `open`、`mkdir`、`remove`、`rename` | 具体操作名 |
| `op_filter` | `open:read`、`open:write`、`open:create`、`provider_open:create` 等 | 带意图的操作名；意图过滤器按它归类 |
| `source` | `path_mapping`、`sandbox_path`、`redirect_root`、`media_provider_java`、`saf_provider` 等 | 路径来源 |
| `from=` / `backend=` | 路径 | 路径被改写时：请求路径与后端实际路径 |
| `caller_uid` / `caller_pid` / `caller_start_time` | 整数 | 发起方身份快照 |
| `correlation_id` | `p<pid>-<seq>` | 请求与文件系统观察的配对标识 |
| `display_from=` / `display_to=` | 路径 | rename 的展示路径与目标路径不同时才有 |

## `action=`：这次操作走没走规则

| 取值 | 含义 |
|---|---|
| `redirect` | 路径被改写，走了重定向；`from=`/`backend=`/`source=` 给出原路径与实际路径 |
| `passthrough` | 这个应用没有配置任何路径规则 |
| `allow` | 有规则，但本次路径不在规则范围内（正常放行） |
| `rule_miss` | 路径落在映射请求范围内，却未被重定向（需要排查） |
| `deny` | 被只读规则拦下，附 `rule=read_only` |

`redirect` / `passthrough` 适用于 `open`、`mkdir`、`unlink`；`allow` 与 `rule_miss` 目前只在
`open` 上细分（其余操作没有经过完整的规则决策，只记录"是否改写路径"）。`rename` 与 SAF
来源记录暂不写该字段。旧版本记录没有该字段，管理 App 按"未知"处理，不显示标签。

## 意图过滤与显示的对应

| `op_filter` 后缀 / `op` | 意图 | UI 文案 |
|---|---|---|
| `:read` | `read` | 读取意图 |
| `:write` | `write` | 写入意图 |
| `:create`、`op=mkdir` | `create` | 创建意图 |
| `:delete`、`op=remove/unlink/rmdir` | `delete` | 删除意图 |
| `:rename`、`op=rename` | `rename` | 重命名意图 |

过滤规则写成 `*:read`、`*:delete` 这类形式；`remove` 等没有 `:delete` 后缀的记录由
操作名兜底归类，同样能被 `*:delete` 命中。
