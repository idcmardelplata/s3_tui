use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
#[allow(clippy::large_enum_variant)]
pub enum AppError {
    #[error("AWS S3 error: {0}")]
    #[allow(clippy::large_enum_variant)]
    S3(#[from] aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::list_buckets::ListBucketsError>),

    #[error("AWS config error: {0}")]
    AwsConfig(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Terminal error: {0}")]
    Terminal(String),

    #[error("Not connected to AWS")]
    NotConnected,

    #[error("No bucket selected")]
    NoBucketSelected,

    #[error("No object selected")]
    NoObjectSelected,

    #[error("Upload failed: {reason}")]
    UploadFailed { reason: String },

    #[error("Download failed: {reason}")]
    DownloadFailed { reason: String },

    #[error("Delete failed: {reason}")]
    DeleteFailed { reason: String },

    #[error("{0}")]
    Custom(String),
}

#[allow(dead_code)]
pub type AppResult<T> = Result<T, AppError>;
