use tonic::Status;

#[derive(Debug, thiserror::Error)]
pub enum PropellerError {
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("failed precondition: {0}")]
    FailedPrecondition(String),

    #[error("internal: {0}")]
    Internal(String),

    #[error("broker error: {0}")]
    Broker(String),
}

impl From<PropellerError> for Status {
    fn from(err: PropellerError) -> Self {
        match err {
            PropellerError::Unauthenticated(msg) => Status::unauthenticated(msg),
            PropellerError::PermissionDenied(msg) => Status::permission_denied(msg),
            PropellerError::InvalidArgument(msg) => Status::invalid_argument(msg),
            PropellerError::NotFound(msg) => Status::not_found(msg),
            PropellerError::FailedPrecondition(msg) => Status::failed_precondition(msg),
            PropellerError::Internal(msg) => Status::internal(msg),
            PropellerError::Broker(msg) => Status::internal(msg),
        }
    }
}
