use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("Timed out while {0}")]
pub struct EmptyTimeoutError(pub String);
