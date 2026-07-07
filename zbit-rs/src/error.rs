// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::fmt;

#[derive(Debug, Clone)]
pub enum ZbitError {
    InvalidArg(&'static str),
    Limit(String),
    Io(String),
    Parse(String),
    Internal(String),
    ValidationMismatch {
        index: usize,
        expected: u8,
        actual: u8,
    },
}

impl fmt::Display for ZbitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArg(msg) => write!(f, "invalid argument: {msg}"),
            Self::Limit(msg) => write!(f, "limit exceeded: {msg}"),
            Self::Io(msg) => write!(f, "i/o error: {msg}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
            Self::ValidationMismatch {
                index,
                expected,
                actual,
            } => write!(
                f,
                "validation mismatch at minterm {index}: expected {expected} got {actual}"
            ),
        }
    }
}

impl std::error::Error for ZbitError {}

impl From<std::io::Error> for ZbitError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

pub type ZbitResult<T> = Result<T, ZbitError>;
