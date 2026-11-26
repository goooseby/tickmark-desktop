# Rust 与 Tauri 桌面端迁移

## 进程边界

`tickmark-capture.exe` 在当前登录用户的 Windows 会话运行。它通过 Microsoft 的 `windows-sys` 绑定调用 Win32，维护前台窗口、输入、会话和电源状态并写入本机 SQLite。采集器无可见窗口，也不加载 Tauri 或 WebView2。

`tickmark-desktop.exe` 提供 Tauri 窗口。默认进入多设备仪表盘，另有今日复盘、趋势和设置；记录明细从时间轴选区、今日复盘或设置进入。关闭界面时采集器继续运行；重新打开窗口可以停表。关闭前需停表时，也可调用旁边的 `tickmark-capture.exe --stop`。采集由用户启动，当前没有开机自启。

采集库使用 `capture.db` 区间表契约，以文件锁限制单进程写入。Tauri 读取 Windows 本机采集、同账号服务器记录和手动导入的设备报告。云端镜像按服务地址和账号隔离，退出后离开报表。CLI 待接入。

## Windows 托盘

打开桌面端时会启动同目录的 tickmark-tray.exe。它只运行 Windows 原生消息循环。关闭界面后可从通知区域重新打开；托盘有自己的单实例保护。Windows 可以把图标收进「隐藏的图标」。

右键菜单显示当前采集状态，并提供打开主窗口、开始或停止采集、打开数据目录、退出。停止采集后托盘继续运行。退出先等待采集器保存并停止，再关闭界面并移除图标。Explorer 恢复通知区域时重新注册图标。托盘启动不替用户开启采集。

## 开发

安装了可用于 Windows MSVC 的 Rust 工具链时，设置 `TICKMARK_RUST_BIN` 为实际包含 `cargo.exe` 和 `rustc.exe` 的工具链目录，或者把路径放在本仓库忽略的 `.tools/rust-bin.txt`。脚本复用本机工具链，缓存放在 `.tools/cargo-cache`，产物放在 `artifacts/rust`。本机验证使用 Keye Desktop 的 Rust 1.98.1、现有 Visual Studio 2022 Build Tools 和 WebView2。

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/rust.ps1 build --workspace --locked
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/rust.ps1 test --workspace --locked
.\artifacts\rust\debug\tickmark-desktop.exe
```

运行时把桌面、采集、托盘三个可执行文件留在同一目录。采用系统 WebView2，界面静态文件随 Tauri 程序编译。前端没有新增 Node 构建步骤。

## 实装到哪一步

- 采集器实装 Win32 前台与标题事件、定时补查、会话和电源通知、固定 120 秒无输入停表、固定 2 秒已识别系统切换防抖。错过的前台边界留出缺口。
- 只对白名单效率工具读取标题并本地清洗。应用名优先读取程序文件版本信息。数据库保存已清洗的标识和区间。
- 媒体豁免只接入浏览器明确上报的视频播放状态和 PowerPoint 放映窗口；普通声音不触发豁免。无法确认视频状态时按 120 秒规则。记录明细固定折叠 5 秒内的短切换；仪表盘时间轴另行平滑 2 秒内的短切换与未识别闪烁。白名单保存在本机 SQLite。
- 墙钟跳变超过 5 秒时重新建立单调时钟与 UTC 的映射。向前出现空段时标为缺口；向后得到的绝对时间与已保存数据冲突时，重叠部分裁掉，避免重复计时。

产品验收口径见 [Windows 应用与页面使用计时设计](windows-usage-capture.md)。

## Edge 网页采集

在 Edge 扩展管理页开启开发人员模式，选择「加载解压缩的扩展」，打开 `native/edge-extension`。用 `scripts/register-edge-host.ps1 -HostExecutable <已构建的 tickmark-browser-host.exe 路径>` 为当前 Windows 用户注册本机通信程序。采集器运行时，设置页会更新连接状态。

扩展读取当前活动标签页，只传普通 HTTP(S) 页面的域名和视频播放状态。路径、查询参数、标题、IP 地址和无痕页面不传；浏览器内置页、文件地址只计应用时长。首个可确认的域名在切入 Edge 后 500 毫秒内收到时，可以补归这段开头时间。同一 Windows 会话存在多个可见 Edge 窗口时，当前版本按应用记录，等待能唯一匹配的窗口后再采集域名。历史记录保持原样。

## 本机验证

2026-11-23 使用 Keye Desktop 的 Rust 工具链完成 Windows workspace 构建，4 项采集与 SQLite 测试通过。实际打开 Tauri 窗口，从界面开启采集，并通过 Windows 输入与前台窗口事件写入一段使用记录。关闭界面后，后台继续运行，随后由独立停止命令退出。关闭界面后的单次 Debug 采样为工作集 16.79 MiB、私有字节 2.57 MiB。锁屏、休眠与放映仍需专项实测。

新版面板的显示口径和截图检查见 [桌面面板与验收记录](dashboard-review.md)。


### 托盘验证

在独立数据目录中实际右键 Windows 通知区的 Tickmark 图标，检查原生菜单。通过同一 Win32 命令入口验证关窗后重开、重复启动时复用主窗口、启停采集时保留托盘、退出时关闭托盘与采集。未重启 Explorer，未对原运行中的数据采集进程发出停止命令。
