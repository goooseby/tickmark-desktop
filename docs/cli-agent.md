# Tickmark CLI · AI 接入说明

这是给自动化脚本和大模型使用的一页契约。完整设计、口径和安装说明见 [命令行界面](cli.md)。

## 基本约束

- 命令名：`tickmark`（安装在用户 PATH）。找不到时用完整路径 `%LOCALAPPDATA%\Tickmark\tickmark.exe`。
- 默认只读、离线，不修改数据库；只有 `import --write` 会写设备报告。
- 默认范围是今天。**每次查询都显式给时间范围**，避免结果随日期漂移。
- 交互式终端默认表格；管道或重定向默认 JSON。给 AI 使用时统一加 `--format json`。
- JSON 字段为 camelCase 英文，`schemaVersion` 当前为 `1`。
- 时间和时长统一用毫秒整数：`*Ms`。另附人类可读的 `duration` / `from` / `to` 仅用于展示。
- 时间默认按系统时区解释；需要固定时区时加 `--tz Asia/Shanghai` 或 `--tz +08:00`。
- 失败信息写 stderr；`--format json` 时 stderr 为 `{"error":{"code":2,"message":"..."}}`。
- 退出码：`0` 成功、`1` 未预期错误、`2` 参数错误、`3` 数据不可用、`4` 请求被拒绝、`5` 配合 `--fail-empty` 的空结果。

## 通用参数

```text
--date YYYY-MM-DD | today | yesterday
--range today | yesterday | week | month
--last 24h | 7d | 30d
--from "2026-11-26 09:00" --to "2026-11-26 18:00"   半开区间 [from, to)
--device all | local | <id> | name:<显示名>          可重复
--cloud [auto|off|PATH]     读取本地云端镜像，默认 off
--format table | json | csv | ndjson
--limit N                   结果条数上限
--top N                     排行榜条数
--fail-empty                空结果返回 5
--tz local | Asia/Shanghai | +08:00
--no-pages                  省略页面标题
```

## 命令

| 命令 | 用途 | 常用参数 |
| --- | --- | --- |
| `tickmark status` | 数据目录、采集器、最后记录、设备数 | — |
| `tickmark devices` | 设备清单、来源、首末记录、区间数 | `--cloud` |
| `tickmark query intervals` | 原始区间，不做平滑合并 | `--state active`、`--app`、`--min-duration 5m`、`--sort duration` |
| `tickmark query apps` | 应用会话，相邻区间合并 | `--merge-apps name`、`--top` |
| `tickmark report` | 与桌面端同口径的汇总 | `--top`、`--merge-apps`、`--split-device` |
| `tickmark aggregate` | 分组聚合 | `--group-by day,device`、`--bucket 15m`、`--metric duration` |
| `tickmark timeline` | 时间轴数据 | `--raw`、`--smooth 2s` |
| `tickmark overlap` | 跨设备并发 | `--top` |
| `tickmark export` | 导出 intervals、apps、report、timeline | `--dataset`、`--out` |
| `tickmark sql` | 内存库只读 SQL | `--file`、`--limit` |
| `tickmark import` | 导入设备报告，默认只检查 | `--write` |
| `tickmark completion` | 生成补全脚本 | `powershell`、`bash`、`zsh`、`fish` |

`aggregate` 的分组键：`day`、`weekday`、`hour`、`device`、`app`、`state`、`page`、`time`；指标：`duration`、`sessions`、`intervals`、`session-avg`。

## 输出示例

`report --date today --format json`：

```json
{
  "schemaVersion": 1,
  "command": "report",
  "generatedAt": "2026-11-26T20:00:00+08:00",
  "range": { "from": "2026-11-26T00:00:00+08:00", "to": "2026-11-27T00:00:00+08:00", "tz": "local" },
  "scope": { "devices": ["local-windows"], "cloud": false },
  "totals": { "activeMs": 6360000, "overlapMs": 0, "deviceSumMs": 6360000 },
  "devices": [
    { "id": "local-windows", "name": "本机电脑", "activeMs": 6360000,
      "coverage": { "activeMs": 6360000, "idleMs": 0, "offlineMs": 0, "gapMs": 0,
                    "unobservedMs": 4164000, "knownMs": 6360000, "elapsedMs": 10524000 } }
  ],
  "apps": [
    { "name": "Trae CN", "durationMs": 2220000, "duration": "37 分", "share": 0.349,
      "devices": ["local-windows"],
      "pages": [{ "name": "report.mjs", "kind": "document", "durationMs": 600000 }] }
  ]
}
```

`query intervals` 的行字段：

```json
{
  "deviceId": "local-windows", "deviceName": "本机电脑",
  "startMs": 1795622400000, "endMs": 1795623600000, "durationMs": 1200000,
  "state": 0, "stateKey": "active",
  "appId": "code.exe", "appName": "Visual Studio Code",
  "page": "report.mjs", "pageKind": "document", "playback": false
}
```

`aggregate` 的行字段：

```json
{ "group": { "day": "2026-11-26" }, "label": "2026-11-26",
  "durationMs": 6360000, "duration": "1 小时 46 分",
  "sessions": 0, "intervals": 0, "share": 1.0 }
```

`timeline` 的活动项与静默段：

```json
{
  "smoothingMs": 2000, "raw": false,
  "items": [{ "deviceId": "local-windows", "startMs": 1795622400000, "endMs": 1795623600000,
              "state": 0, "stateKey": "active", "appId": "code.exe", "appName": "Visual Studio Code" }],
  "quiet": [{ "startMs": 1795623600000, "endMs": 1795627200000, "reason": "idle", "compressed": true }]
}
```

`quiet.reason`：`sleep`、`idle`、`offline`、`gap`、`no-activity`、`no-record`、`future`。

## SQL 内存库

`tickmark sql` 只接受一条 `SELECT` / `WITH` 语句，禁止写入和 `PRAGMA`。

| 表 | 列 |
| --- | --- |
| `devices` | `id`、`name`、`kind`、`platform`、`source`、`first_ms`、`last_ms` |
| `intervals` | `device_id`、`start_ms`、`end_ms`、`state`、`state_key`、`app_id`、`app_name`、`page`、`page_kind`、`playback` |
| `app_sessions` | `device_id`、`app_id`、`app_name`、`start_ms`、`end_ms` |

示例：

```sql
select app_name, round(sum(end_ms - start_ms) / 60000.0, 1) as minutes
from app_sessions
group by app_name
order by minutes desc
```

## 跨设备

- 默认只读本机 `capture.db` 和 `report-devices/*.json`。
- 其他设备在本地云端镜像里，加 `--cloud` 才参与计算；镜像不会联网刷新。
- 判断当前有哪些设备：`tickmark devices --cloud --format json`。
- 跨设备总用时是去重并集；`deviceSumMs` 是各设备之和，两者差额是 `overlapMs`。

## 给 AI 的系统提示

```text
你可以使用 tickmark 命令行读取本机使用记录。规则：
1. 先运行 tickmark status 确认数据库存在；需要其他设备时加 --cloud。
2. 每次查询都显式指定 --date、--range、--last 或 --from/--to，并加 --format json。
3. 只读使用，不要运行 import --write，不要修改数据目录。
4. 时间戳是 Unix 毫秒；带 Ms 后缀的字段是毫秒，duration 只是展示文本。
5. 结论必须来自命令输出；数据为空时说明为空，不要编造。
6. 命令失败时读取 stderr 的 error.message，并根据退出码判断是参数、数据还是权限问题。
```

## 示例

```powershell
tickmark report --date today --format json
tickmark query apps --range week --top 10 --merge-apps name --format json
tickmark aggregate --range month --group-by day,device --format json
tickmark query intervals --date today --state active --min-duration 5m --format json
tickmark overlap --range week --cloud --format json
tickmark export --range month --dataset apps --format csv --out apps.csv
```
