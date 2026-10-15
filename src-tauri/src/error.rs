use serde::Serialize;
use specta::Type;

/// The single error type returned by every command.
/// Serialises to `{ kind, message }` for the frontend.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Parse(String),
    #[error("{0}")]
    Store(String),
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AppErrorKind {
    NotFound,
    Io,
    Parse,
    Store,
}

#[derive(Serialize, Type)]
#[serde(rename = "AppError")]
pub struct AppErrorPayload {
    pub kind: AppErrorKind,
    pub message: String,
}

impl AppError {
    pub fn io(context: impl std::fmt::Display, err: std::io::Error) -> Self {
        Self::Io(format!("{context}: {err}"))
    }

    fn payload(&self) -> AppErrorPayload {
        let kind = match self {
            Self::NotFound(_) => AppErrorKind::NotFound,
            Self::Io(_) => AppErrorKind::Io,
            Self::Parse(_) => AppErrorKind::Parse,
            Self::Store(_) => AppErrorKind::Store,
        };
        AppErrorPayload { kind, message: self.to_string() }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.payload().serialize(serializer)
    }
}

impl Type for AppError {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        AppErrorPayload::definition(types)
    }
}

pub type AppResult<T> = Result<T, AppError>;
