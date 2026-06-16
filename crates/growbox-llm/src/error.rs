use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("HTTP 请求失败: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API 返回错误 {status}: {body}")]
    Api { status: u16, body: String },
    #[error("响应解析失败: {0}")]
    Parse(String),
    #[error("配置错误: {0}")]
    Config(String),
}

pub type LlmResult<T> = Result<T, LlmError>;
