use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
#[allow(clippy::large_enum_variant)]
pub enum AppError {
    #[error("Error de AWS S3: {0}")]
    #[allow(clippy::large_enum_variant)]
    S3(#[from] aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::list_buckets::ListBucketsError>),

    #[error("Error de configuración AWS: {0}")]
    AwsConfig(String),

    #[error("Error de E/S: {0}")]
    Io(#[from] std::io::Error),

    #[error("Error de terminal: {0}")]
    Terminal(String),

    #[error("Sin conexión a AWS")]
    NotConnected,

    #[error("No se seleccionó un bucket")]
    NoBucketSelected,

    #[error("No se seleccionó un objeto")]
    NoObjectSelected,

    #[error("Error al subir: {reason}")]
    UploadFailed { reason: String },

    #[error("Error al descargar: {reason}")]
    DownloadFailed { reason: String },

    #[error("Error al borrar: {reason}")]
    DeleteFailed { reason: String },

    #[error("{0}")]
    Custom(String),
}

#[allow(dead_code)]
pub type AppResult<T> = Result<T, AppError>;
