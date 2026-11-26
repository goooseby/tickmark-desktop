use crate::{errors::CliError, output};

const COMMANDS: &str = "status devices query report aggregate timeline overlap export sql import path completion";

pub fn run(shell: &str) -> Result<bool, CliError> {
    let script = match shell.trim().to_ascii_lowercase().as_str() {
        "powershell" | "pwsh" => format!(
            "Register-ArgumentCompleter -Native -CommandName tickmark -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)
    '{COMMANDS}'.Split(' ') |
        Where-Object {{ $_ -like \"$wordToComplete*\" }} |
        ForEach-Object {{
            [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
        }}
}}
"
        ),
        "bash" => format!("complete -W \"{COMMANDS}\" tickmark\n"),
        "zsh" => format!(
            "#compdef tickmark\n_arguments '1: :({COMMANDS})' '*::arg:->args'\n"
        ),
        "fish" => {
            let lines: Vec<String> = COMMANDS
                .split(' ')
                .map(|command| format!("complete -c tickmark -f -a {command}"))
                .collect();
            lines.join("\n") + "\n"
        }
        other => {
            return Err(CliError::usage(format!(
                "无法识别的 shell「{other}」，可用 powershell、bash、zsh、fish。"
            )));
        }
    };
    output::print_block(&script);
    Ok(false)
}
