use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Sled database error: {0}")]
    Db(#[from] sled::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] SerializationError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Error, Debug)]
pub enum SerializationError {
    #[error("Invalid data: {0}")]
    InvalidData(&'static str),
}
