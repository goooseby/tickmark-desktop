use std::fmt;

#[derive(Debug, Clone)]
pub struct CliError {
    pub code: i32,
    pub message: String,
}

impl CliError {
    pub fn error(message: impl Into<String>) -> Self {
        Self { code: 1, message: message.into() }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self { code: 2, message: message.into() }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self { code: 3, message: message.into() }
    }

    pub fn rejected(message: impl Into<String>) -> Self {
        Self { code: 4, message: message.into() }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl From<String> for CliError {
    fn from(message: String) -> Self {
        Self::error(message)
    }
}

impl From<&str> for CliError {
    fn from(message: &str) -> Self {
        Self::error(message)
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::error(error.to_string())
    }
}
