mod cli;
mod commands;
mod completion;
mod data;
mod errors;
mod output;
mod path;
mod sql;
mod time;

use clap::Parser;

use cli::{Cli, Command, QueryKind};
use errors::CliError;

fn main() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
    }
    let cli = Cli::parse();
    let zone = match time::Zone::parse(&cli.global.tz) {
        Ok(zone) => zone,
        Err(message) => {
            eprintln!("tickmark: {message}");
            std::process::exit(2);
        }
    };
    let result: Result<bool, CliError> = match &cli.command {
        Command::Status => commands::status(&cli.global, &zone),
        Command::Devices => commands::devices(&cli.global, &zone),
        Command::Query { kind } => match kind {
            QueryKind::Intervals(args) => commands::query_intervals(&cli.global, &zone, args),
            QueryKind::Apps(args) => commands::query_apps(&cli.global, &zone, args),
        },
        Command::Report(args) => commands::report(&cli.global, &zone, args),
        Command::Aggregate(args) => commands::aggregate(&cli.global, &zone, args),
        Command::Timeline(args) => commands::timeline(&cli.global, &zone, args),
        Command::Overlap(args) => commands::overlap(&cli.global, &zone, args),
        Command::Export(args) => commands::export(&cli.global, &zone, args),
        Command::Sql(args) => sql::run(&cli.global, &zone, args),
        Command::Import(args) => commands::import(&cli.global, &zone, args),
        Command::Path(args) => path::run(args, cli.global.format.as_deref()),
        Command::Completion(args) => completion::run(&args.shell),
    };
    match result {
        Ok(empty) => {
            if empty && cli.global.fail_empty {
                std::process::exit(5);
            }
        }
        Err(error) => {
            if cli.global.format.as_deref() == Some("json") {
                let payload = serde_json::json!({
                    "error": { "code": error.code, "message": error.message }
                });
                eprintln!("{payload}");
            } else {
                eprintln!("tickmark: {}", error.message);
            }
            std::process::exit(error.code);
        }
    }
}
