use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("Timed out while {0}")]
pub struct TimeoutError(pub String);

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("Connection to {0} was closed")]
pub struct ConnectionClosedError(pub String);
