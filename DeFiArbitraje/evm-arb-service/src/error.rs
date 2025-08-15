use thiserror::Error;

#[derive(Error, Debug)]
pub enum ArbError {
    #[error("Config error: {0}")]
    Config(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Quote error: {0}")]
    Quote(String),

    #[error("Execution error: {0}")]
    Execution(String),
}
