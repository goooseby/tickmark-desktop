use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tickmark",
    version = concat!(env!("CARGO_PKG_VERSION"), " (schema 1)"),
    about = "Tickmark 命令行：查询、聚合与导出本机使用记录",
    long_about = "Tickmark 命令行与桌面版读取同一份数据。默认只读、离线，不修改数据库。",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Clone)]
pub struct Global {
    /// 数据目录，默认 %LOCALAPPDATA%\TickmarkDesktop
    #[arg(long, global = true, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,
    /// 是否读取本地云端镜像：auto、off 或镜像路径
    #[arg(
        long,
        global = true,
        value_name = "auto|off|PATH",
        num_args = 0..=1,
        default_missing_value = "auto"
    )]
    pub cloud: Option<String>,
    /// 输出格式：table、json、csv、ndjson
    #[arg(long, global = true, value_name = "table|json|csv|ndjson")]
    pub format: Option<String>,
    /// 结果条数上限
    #[arg(long, global = true, value_name = "N")]
    pub limit: Option<usize>,
    /// 结果为空时返回退出码 5
    #[arg(long = "fail-empty", global = true)]
    pub fail_empty: bool,
    /// 表格不截断长文本
    #[arg(long, global = true)]
    pub wide: bool,
    /// 省略页面与文档标题
    #[arg(long = "no-pages", global = true)]
    pub no_pages: bool,
    /// 时区：local、IANA 名称或 +08:00
    #[arg(long, global = true, value_name = "local|Asia/Shanghai|+08:00", default_value = "local")]
    pub tz: String,
    /// 允许超过 366 天的范围
    #[arg(long = "allow-large", global = true)]
    pub allow_large: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// 数据目录、采集器和记录概览
    Status,
    /// 列出设备、来源和覆盖范围
    Devices,
    /// 查询原始区间或应用会话
    Query {
        #[command(subcommand)]
        kind: QueryKind,
    },
    /// 与仪表盘同口径的汇总
    Report(ReportArgs),
    /// 按维度聚合时长或会话
    Aggregate(AggregateArgs),
    /// 与界面一致的时间轴数据
    Timeline(TimelineArgs),
    /// 跨设备并发使用
    Overlap(OverlapArgs),
    /// 导出区间、会话、汇总或时间轴
    Export(ExportArgs),
    /// 在内存库中执行只读 SQL
    Sql(SqlArgs),
    /// 导入设备区间报告
    Import(ImportArgs),
    /// 维护用户 PATH 中的 Tickmark 目录
    Path(PathArgs),
    /// 输出 shell 补全脚本
    Completion(CompletionArgs),
}

#[derive(Subcommand)]
pub enum QueryKind {
    /// 原始区间，不做平滑和合并
    Intervals(IntervalsArgs),
    /// 应用会话，按设备 + 应用合并相邻区间
    Apps(AppsArgs),
}

#[derive(Args, Clone, Default)]
pub struct RangeArgs {
    /// 起始时间，包含
    #[arg(long, value_name = "时间")]
    pub from: Option<String>,
    /// 结束时间，不包含
    #[arg(long, value_name = "时间")]
    pub to: Option<String>,
    /// 单日，等价于本地零点到次日零点
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub date: Option<String>,
    /// 常用范围：today、yesterday、week、month
    #[arg(long, value_name = "today|yesterday|week|month")]
    pub range: Option<String>,
    /// 滚动窗口，例如 24h、7d、30d
    #[arg(long, value_name = "24h")]
    pub last: Option<String>,
}

#[derive(Args, Clone)]
pub struct IntervalsArgs {
    #[command(flatten)]
    pub range: RangeArgs,
    /// 设备：all、local、id 或 name:名称，可重复
    #[arg(long, value_name = "设备")]
    pub device: Vec<String>,
    /// 状态：active、idle、offline、gap，可重复
    #[arg(long, value_name = "状态")]
    pub state: Vec<String>,
    /// 应用名称或标识，可重复
    #[arg(long, value_name = "应用")]
    pub app: Vec<String>,
    /// 只看未识别应用
    #[arg(long = "app-unidentified")]
    pub app_unidentified: bool,
    /// 页面或文档标题包含的文本
    #[arg(long = "page-contains", value_name = "文本")]
    pub page_contains: Option<String>,
    /// 只看媒体播放区间
    #[arg(long)]
    pub playback: bool,
    /// 最短时长，例如 5m
    #[arg(long = "min-duration", value_name = "时长")]
    pub min_duration: Option<String>,
    /// 排序：start 或 duration
    #[arg(long, value_name = "start|duration")]
    pub sort: Option<String>,
}

#[derive(Args, Clone)]
pub struct AppsArgs {
    #[command(flatten)]
    pub range: RangeArgs,
    /// 设备：all、local、id 或 name:名称，可重复
    #[arg(long, value_name = "设备")]
    pub device: Vec<String>,
    /// 应用名称或标识，可重复
    #[arg(long, value_name = "应用")]
    pub app: Vec<String>,
    /// 同名应用是否跨设备合并：none 或 name
    #[arg(long = "merge-apps", value_name = "none|name", default_value = "none")]
    pub merge_apps: String,
    /// 按指标排序并取前 N
    #[arg(long, value_name = "N")]
    pub top: Option<usize>,
}

#[derive(Args, Clone)]
pub struct ReportArgs {
    #[command(flatten)]
    pub range: RangeArgs,
    /// 设备：all、local、id 或 name:名称，可重复
    #[arg(long, value_name = "设备")]
    pub device: Vec<String>,
    /// 应用和页面条数
    #[arg(long, value_name = "N", default_value_t = 10)]
    pub top: usize,
    /// 同名应用是否跨设备合并：none 或 name
    #[arg(long = "merge-apps", value_name = "none|name", default_value = "name")]
    pub merge_apps: String,
    /// 额外输出每台设备的应用榜
    #[arg(long = "split-device")]
    pub split_device: bool,
}

#[derive(Args, Clone)]
pub struct AggregateArgs {
    #[command(flatten)]
    pub range: RangeArgs,
    /// 设备：all、local、id 或 name:名称，可重复
    #[arg(long, value_name = "设备")]
    pub device: Vec<String>,
    /// 分组：day、weekday、hour、device、app、state、page、time，逗号分隔
    #[arg(long = "group-by", value_name = "day,device", default_value = "day")]
    pub group_by: String,
    /// 绝对时间桶：5m、15m、30m、1h、1d
    #[arg(long, value_name = "15m")]
    pub bucket: Option<String>,
    /// 指标：duration、sessions、intervals、session-avg
    #[arg(long, value_name = "duration", default_value = "duration")]
    pub metric: String,
    /// 只看这些状态，默认 active
    #[arg(long, value_name = "active|idle|offline|gap|all", default_value = "active")]
    pub state: String,
    /// 按指标取前 N 组
    #[arg(long, value_name = "N")]
    pub top: Option<usize>,
    /// 排序：metric 或 key
    #[arg(long, value_name = "metric|key", default_value = "metric")]
    pub sort: String,
}

#[derive(Args, Clone)]
pub struct TimelineArgs {
    #[command(flatten)]
    pub range: RangeArgs,
    /// 设备：all、local、id 或 name:名称，可重复
    #[arg(long, value_name = "设备")]
    pub device: Vec<String>,
    /// 关闭平滑、折叠和时间轴压缩
    #[arg(long)]
    pub raw: bool,
    /// 平滑阈值，默认 2s
    #[arg(long, value_name = "2s", default_value = "2s")]
    pub smooth: String,
}

#[derive(Args, Clone)]
pub struct OverlapArgs {
    #[command(flatten)]
    pub range: RangeArgs,
    /// 设备：all、local、id 或 name:名称，可重复
    #[arg(long, value_name = "设备")]
    pub device: Vec<String>,
    /// 输出的并发时段条数
    #[arg(long, value_name = "N", default_value_t = 3)]
    pub top: usize,
}

#[derive(Args, Clone)]
pub struct ExportArgs {
    #[command(flatten)]
    pub range: RangeArgs,
    /// 设备：all、local、id 或 name:名称，可重复
    #[arg(long, value_name = "设备")]
    pub device: Vec<String>,
    /// 数据集：intervals、apps、report、timeline
    #[arg(long, value_name = "intervals", default_value = "intervals")]
    pub dataset: String,
    /// 输出文件，- 表示标准输出
    #[arg(long, value_name = "PATH")]
    pub out: Option<String>,
    /// CSV 不写 BOM
    #[arg(long = "no-bom")]
    pub no_bom: bool,
    /// 导出前按应用筛选
    #[arg(long, value_name = "应用")]
    pub app: Vec<String>,
}

#[derive(Args, Clone)]
pub struct SqlArgs {
    /// 只读 SQL，单条语句
    #[arg(value_name = "SQL")]
    pub query: Option<String>,
    /// 从文件读取 SQL
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
    #[command(flatten)]
    pub range: RangeArgs,
    /// 设备：all、local、id 或 name:名称，可重复
    #[arg(long, value_name = "设备")]
    pub device: Vec<String>,
    /// 结果条数上限，默认 1000
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

#[derive(Args, Clone)]
pub struct ImportArgs {
    /// 设备区间报告（JSON）
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// 写入报告；默认只检查
    #[arg(long)]
    pub write: bool,
    /// 显式只检查，与 --write 互斥
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Args, Clone)]
pub struct PathArgs {
    /// 只查看当前 PATH 中是否包含 Tickmark 目录
    #[arg(long)]
    pub status: bool,
    /// 把目录加入用户 PATH
    #[arg(long, value_name = "PATH")]
    pub add: Option<String>,
    /// 从用户 PATH 移除目录
    #[arg(long, value_name = "PATH")]
    pub remove: Option<String>,
}

#[derive(Args, Clone)]
pub struct CompletionArgs {
    /// powershell、bash、zsh 或 fish
    #[arg(value_name = "SHELL")]
    pub shell: String,
}
