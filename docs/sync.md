# Tickmark 跨端同步计划

状态：后端已部署在阿里云 `8.138.204.47`，生产 HTTPS 基地址为 `https://kovra.top/tickmark`。账号协议、Android 上传与桌面收发已实现。首个受邀地址已启用，生产验证码已确认投递至 QQ 邮箱服务。桌面仍可读取独立的本地报告文件。

## SYNC-01 拓扑和账号

Android 手机、平板和 Windows 电脑先在本机采集。本地报表可以离线使用。Android 管理自己的历史、上传范围、队列和本机报表，只上传自身数据。Windows 上传本机数据并读取同账号所有设备的数据，桌面报表本地计算合并和并发。云端在提交事务后确认，保留每个账号、设备、UTC 日的最新修订和删除标记。上传不依赖任何电脑在线。新电脑从零读取已有历史，现有电脑按游标补拉。

首版建议采用**邮箱验证码登录、受邀邮箱开通**。后台配置允许的首批邮箱，验证成功后创建账号。账号负责归属、设备列表和权限，设备使用单独的随机安装 ID。用户可命名设备，类别和平台只用于展示。设备撤销后立即停止其读写，本地采集仍可继续。正式开放注册前，再评估运营、账号恢复和治理需求。

验证码 6 位、5 分钟有效、一次使用、单次挑战最多 5 次尝试，发送按邮箱和 IP 分别限频，初始重发间隔 60 秒。数据库保存绑定挑战和邮箱的验证码 HMAC，密钥由服务端单独保管。失败响应避免暴露邮箱是否已注册。验证码通过后绑定设备，签发 15 分钟访问令牌和可轮换、可撤销的设备刷新令牌。设备刷新互斥，响应丢失时提供短时的同请求幂等重试，重复使用已作废令牌触发会话处理。所有 API 根据令牌确定账号，写入时服务端另行验证设备归属。不同账号无法互读，设备只能写本设备分片。

### Windows 账号接口

下列接口均为 JSON POST，请求字段使用 camelCase。成功会话包含 `account: { id, email }`、`accessToken`、`accessExpiresInSeconds` 和 `refreshToken`。邮箱匹配使用去掉首尾空白后的 ASCII 小写地址，登录不要求密码。验证码是六位字符串，保留前导零。

| 路径 | 请求 | 成功行为 |
| --- | --- | --- |
| `/v1/auth/challenges` | `email` | 返回 `challengeId`、`expiresInSeconds`、`retryAfterSeconds`。受邀与未受邀邮箱使用一致的外部响应。 |
| `/v1/auth/verify` | `email`、`challengeId`、`code`、`device: { id, name, platform, kind }` | 核验后返回会话，绑定随机安装 UUID。本机默认设备名为 Windows 计算机名，平台为 `windows`，类别为 `computer`。 |
| `/v1/auth/refresh` | `refreshToken`、`requestId` | 刷新并轮换会话。同一刷新令牌与请求 UUID 的重试返回同一次轮换结果。 |
| `/v1/auth/logout` | `refreshToken` | 返回 HTTP 204，撤销该令牌所属的登录会话。若客户端在轮换回包丢失后退出，轮换前的令牌也能定位并撤销同一会话。重复退出须幂等。 |

业务错误返回 `{ "code": "…" }`。桌面处理 `invalid_code`、`challenge_expired`、`challenge_exhausted`，会话撤销和刷新无效统一走 401/403，错误码为 `session_revoked`、`invalid_refresh` 或 `device_revoked`。限流返回 HTTP 429 和以秒表示的 `Retry-After`。后续的设备撤销使用独立的 `POST /v1/devices/{id}/revoke`，用于移除其他设备。

桌面左下角显示账号头像和邮箱，完整邮箱可在浮层查看。账号菜单向上打开，提供邮箱登录、设置和退出。登录弹窗同时显示邮箱和验证码。发送和重发使用验证码旁的按钮，验证码只可用于请求时填写的邮箱。关闭或登录成功后回到原先的报表或设置。旁边的齿轮和 `Ctrl+,` 打开「设置」，采集状态显示在设置页的采集卡片中。

桌面用当前 Windows 用户的 DPAPI 加密保存账号摘要、安装 UUID 和刷新令牌，按 API 地址隔离，原子落盘。访问令牌只驻留在 Rust 内存，WebView 只读取账号摘要与状态。启动时尝试恢复会话，刷新异常时显示离线状态，服务端明确撤销时清除登录。退出先持久化本机退出结果，服务器暂时不可达时，重新联网或下次启动会重试撤销旧会话；同一服务器新登录前会先完成待撤销请求。登录成功不触发历史记录上传。

桌面端默认连接 `https://kovra.top/tickmark`，普通重编译会继续使用该地址对应的账号文件和同步缓存。其他部署可在构建时通过 `TICKMARK_ACCOUNT_API` 覆盖 HTTPS 地址。调试构建可用 `TICKMARK_ACCOUNT_API_TEST` 指向测试服务，HTTP 仅允许回环地址，测试时可传空值验证未配置状态。验证命令为 `./scripts/rust.ps1 build -p tickmark-desktop` 和 `node native/tests/account-webview.mjs`，截图写入 `artifacts/account-review/`。

## SYNC-02 邮件是谁发的，怎么开通

2026-11-24 已在阿里云华东（杭州）开通 DirectMail，发信子域 `tickmark-mail.kovra.top` 的 DKIM、SPF、DMARC 和 MX 已通过控制台验证。2026-11-25，生产后端通过 SMTPS 启用触发型发信地址 `login@tickmark-mail.kovra.top`。同日通过生产登录接口发出的验证码在阿里云控制台显示投递成功，详情为 `250 Send Mail OK`。实际登录由收件人使用客户端新请求的验证码完成。

邮件显示发件人「Tickmark」和 `login@tickmark-mail.kovra.top`。后端使用 `smtpdm.aliyun.com:465`、TLS 和 SMTP LOGIN，将验证码邮件交给阿里云投递。发信凭据仅保存在服务端的 root 配置中。 [开通和发信流程](https://help.aliyun.com/zh/direct-mail/getting-started/simplified-procedure-of-sending-by-api-and-smtp) · [发信地址设置](https://help.aliyun.com/zh/direct-mail/user-guide/setup-sender-addresses/)

1. 有自有域名就使用其独立子域。没有则注册一个项目域名，完成平台所需的实名核验。API 用 `api.项目域名`，邮件用 `mail.项目域名`，两者各配 DNS；正式 API 使用受信任的 HTTPS 证书。
2. 在 DirectMail 创建发信子域，按控制台给出的准确记录完成验证。阿里云当前要求新发信域名通过 SPF、DKIM、DMARC、MX 四项校验。创建触发型发信地址，绑定验证邮件的用途。 [发信域名配置](https://help.aliyun.com/zh/direct-mail/user-guide/how-to-configure-sending-domain-names) · [邮件投递说明](https://help.aliyun.com/zh/direct-mail/user-guide/summary-of-common-email-return-codes-and-solutions)
3. 为项目发信地址设置独立 SMTP 凭据，在服务器配置凭据及受邀邮箱。后端日志记录阿里云接收或拒绝的状态，控制台「发送详情」用于核对最终投递结果。
4. 若还没有域名，可先在本地用测试邮件接收器验证登录。DirectMail 提供公共域代发的自定义地址选项，但阿里云提示容易遇到拒收或垃圾邮件判定，实际可用性还取决于账号控制台，首版上线建议用自有域。 [发信地址设置](https://help.aliyun.com/zh/direct-mail/user-guide/setup-sender-addresses/)

备案与邮件开通分开核验。若服务器在中国内地并对外提供 Web/API 服务，先按阿里云规则确认域名备案、是否需要接入备案及该实例是否满足条件。若对外提供 Android 联网同步，另核对 APP 备案及发布渠道要求。域名和发信验证本身无法替代 APP 备案。对外提供时需准备隐私告知、删除和退出入口，按适用规则核对使用轨迹、邮箱和第三方邮件投递的处理范围。 [阿里云备案说明](https://help.aliyun.com/zh/icp-filing/basic-icp-service/product-overview/faq-about-icp-filing-applications-in-different-scenarios) · [工信部 APP 备案通知](https://www.miit.gov.cn/zwgk/zcwj/wjfb/tz/art/2023/art_920db564162e4312916a01bed6540ad8.html) · [个人信息保护法](https://www.cac.gov.cn/2021-08/20/c_1631050028355286.htm)

## SYNC-03 发送的数据与统计对齐

| 项 | 规则 |
| --- | --- |
| 主键 | `accountId + deviceId + utcDay`。协议用 UTC 自然日做传输分片。报表再按用户显示时区切日，所以旅行、夏令时和修改时区均能重算。 |
| 时间 | Unix UTC 毫秒，区间左闭右开。跨 UTC 午夜的段先裁剪再分片。切分前后时长守恒。客户端按原始墙钟与来源时钟规则生成，服务器记录提交时间并校验边界。 |
| 设备状态 | `active / idle / offline / gap`，每设备互斥。Windows 固定采用前台应用、120 秒无操作停表；仅前台视频正在播放或 PowerPoint 全屏放映时豁免，音乐、语音会议和普通有声应用不豁免，检测不到明确播放状态时按 120 秒规则。Android 对齐为可交互且已解锁，不增加 Windows 的空闲和视频规则，并保留来源口径差异。资料不足的区间保持未知或缺口。 |
| 应用会话 | 按 `appId` 保存前台起止和 `observed / inferred`。Android 同应用重叠去重，不同应用的分屏并行保留。设备活跃只计一次，各应用各计自己的区间。 |
| 标识与页面 | Android 只使用应用包名，不采集网页域名。Windows 使用现有应用标识；Edge 额外上传域名，其他浏览器首版只上传应用时长。只在维护明确跨端映射后合并同款应用，同名不足以合并。域名不包含路径、查询参数或标题；Windows 文档/项目名默认留在本机，开启单独授权后才能发送。 |
| 历史汇总 | 只有应用总量的 Android 系统桶留在本机或用独立的粗粒度通道，不能伪造成小时级会话，不能参与精确重叠。 |
| 隐私 | 本地采集权限与同步开关独立。同步使用经过隐私过滤的区间，不发送原始系统事件、标题原文、完整网址、无痕页面、写作内容、复盘或备注。隐藏及排除应用不上传身份。启用时明确初始上送范围，历史全量只作为显式操作。 |

报表按各设备活跃的并集计算去重用时。至少两台设备同时活跃的并集为重叠时长，分屏自身不增加设备并发数。不同设备采集定义随数据标记，缺设备、缺时段、观测为零分别表达。Windows 的 120/2/5 计时与展示阈值是统一产品规则，不作为普通用户选项；可调项仅限隐私、同步范围和设备管理。 [报表规则](reports.md) · [Windows 计时规则](windows-usage-capture.md)

## SYNC-04 版本、上行和下行

1. **本机提交**。每台设备将变动的 UTC 日标为待同步。分片内容由本机记录重算；版本使用持久化递增整数，系统时钟回拨不改变版本排序。发出的版本和载荷固定，等待确认时若又有修改就生成更高版本。保留待发送记录至确认后再清理。
2. **上行接口**。`PUT /v1/devices/{deviceId}/shards` 接收 `epoch` 和小批量 UTC 日修订。每个分片包括 `dayUtc`、`revision`、`payloadVersion` 与 `payload`；`payload:null` 表示删除。服务器先验证账号、设备、范围、大小和内容，再事务写入。已确认的较低版本返回过时；同版本同内容确认重试；同版本不同内容返回 409，批次无副作用。最终 CI 样本覆盖请求已提交但响应丢失的场景。
3. **下行接口**。Windows 专用的 `GET /v1/changes?after=&limit=&epoch=` 返回 `epoch`、`devices[]`、按提交序号排序的 `changes[]`、`nextAfter` 与 `hasMore`。每页最多 8 个分片。服务端为每个键保存最新修订，新电脑从序号零补齐。设备资料与分片、游标一起提交到本地账号缓存后可见。当前设备的云端副本不进入本机统计。
4. **补齐与恢复**。世代不匹配返回 410。桌面清除旧世代的可见云端缓存并从零补拉，Android 通过 `GET /v1/sync/status` 重新获取世代，再从本机已授权范围重建自身上传。各设备的修订计数保持递增。恢复备份时管理员重新生成世代，使各端重新对齐。
5. **修订和删除**。及时上送隐私收紧后的修订；若关闭同步，停止传输并提示已同步数据的处置方式。移除历史或设备可由用户选择同步删除。删除版本保留最小元数据，同一旧版本无法重新创建数据。整个账号删除时撤销所有令牌，删除云端数据，并按备份保留规则到期移除。换账号时清空上一账号可见的远端镜像与会话，首次上传前确认哪些本机历史属于新账号。

下面是拟定的上行样本。实际载荷需要验证窗口边界、时间排序和支持的版本。`seq` 由服务器分配，客户端上传时无需提供。下行复用同一分片结构并附带 `deviceId`、`seq` 和服务器接收时间。

```json
{
  "epoch": "e1",
  "shards": [
    {
      "dayUtc": "2026-11-24",
      "revision": 7,
      "payloadVersion": 1,
      "payload": {
        "zoneId": "Asia/Shanghai",
        "captureProfile": "android.interactive-unlocked.v1",
        "deviceIntervals": [
          {"startMs": 1795478400000, "endMs": 1795478460000, "state": "active"}
        ],
        "appIntervals": [
          {"startMs": 1795478400000, "endMs": 1795478460000, "appId": "example.reader", "quality": "observed"}
        ]
      }
    }
  ]
}
```

服务器拒绝越权、非法区间和不支持的版本。世代失效与异版本内容冲突使用不同的机器可读错误码。限流响应带重试时间。客户端收到确认后再推进上传队列，收到下行记录且本地事务完成后再推进读取游标。通信使用 HTTPS。

## SYNC-05 自动同步节奏

| 客户端 | 运行方式 |
| --- | --- |
| Android 应用打开 | 登录状态有效时立即同步，前台每 30 秒检查一次。稳定网络下，记录落本后用约 5 秒合并小批更新，再触发同步。退到后台取消界面循环。 |
| Android 后台 | 复用 WorkManager，账号内仅一个定期同步，联网约束，计划间隔初设 15 分钟。恢复联网或本机产生待发送时调度去重的一次性任务。后台受 Doze、强制停止、厂商限制影响，恢复前台立即补齐。 [Android WorkManager 调度限制](https://developer.android.com/reference/androidx/work/PeriodicWorkRequest) |
| Windows | 桌面窗口进程每约 5 秒检查云端，并上传从首次成功连接时起的本机记录。网络错误退避至 60 秒。关闭桌面窗口后，采集进程仍可继续，重开窗口再补传和补拉。 |

同步流程互斥。网络错误指数退避并加随机抖动，复网后主动恢复。界面只展示最近成功时间、未发送状态、连接或登录问题和设备列表。首轮验收目标为稳定网络下，从本地记录提交到另一台在线设备可见，95% 在 30 秒内；Android 后台另测实际分布。

## SYNC-06 服务部署与交付次序

首版后端采用 **Python + Starlette/Uvicorn + SQLite WAL**，部署源码在独立本地仓库 `D:\workspace\Tickmark-Server`。选用服务器现有的 Python 3.10 减少上线工具链，独立虚拟环境和 systemd 服务。Nginx 复用现有受信任 HTTPS，后端只监听 `127.0.0.1:17811`。数据库和备份独立于现有服务及客户端采集库。每天进行 SQLite 在线备份及校验，留最近 14 份。服务路径与恢复步骤见后端仓库 README。

1. **契约样本**。先固定 Android、Windows 三台设备的共享 JSON 样本和错误响应。覆盖独立设备身份、UTC 切片、分屏、缺口、同版本冲突、时区变化、删除和旧世代。
2. **本地后端**。实现账号、验证码、设备绑定、SQLite 分片和变更流。用本地邮件接收器验证密码学、限流、撤销、跨账号隔离和数据库恢复。
3. **客户端闭环**。启动桌面保持其本机功能，再验证 Android 接入、Windows 接收和 Windows 上传。测试两台 Android 和一台 Windows 同账号运行、离线后回补、应用进前后台、重装和服务器恢复。
4. **服务器上线**。核验服务器系统、网络、DNS、HTTPS、域名及 APP 备案适用情况。配置邮件触发地址和服务权限，部署后做真实投递与小范围同步验证，再记录版本、回滚步骤和备份路径。

实际部署已验证 HTTPS 健康检查及无凭据读取返回 401。首个允许登录的收件邮箱和发信凭据尚未配置。Android 接入步骤见其仓库的 `docs/15-cross-device-sync.md`。

2026-11-25 本地闭环使用真实 Starlette HTTP 服务和持久化 SQLite，加上模拟邮箱。Android API 36 模拟器先上传自己的区间，两份独立 Windows 安装随后从同一账号拉取并打开该设备的应用流水。另验证 Windows 产生的新区间写入服务端数据库、退出账号后隐藏云端数据。实际生产投递与真机后台行为分开验收。
