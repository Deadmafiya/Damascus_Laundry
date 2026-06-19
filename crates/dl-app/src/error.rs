//! Live execution error types (v1.1+).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LiveError {
    #[error("pipeline error: {0}")]
    Pipeline(String),

    #[error("bundle assembly error: {0}")]
    Bundle(String),

    #[error("Jito error: {0}")]
    Jito(String),

    #[error("I/O error: {0}")]
    Io(String),
}
