//! Error types for the radish library.

use thiserror::Error;

/// Result type alias for radish operations
pub type Result<T> = std::result::Result<T, RadishError>;

/// Main error type for radish operations
#[derive(Error, Debug)]
pub enum RadishError {
    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// HDF5 error
    #[error("HDF5 error: {0}")]
    Hdf5(#[from] hdf5::Error),

    /// NetCDF error
    #[error("NetCDF error: {0}")]
    NetCdf(#[from] netcdf::Error),

    /// File format error
    #[error("Invalid file format: {0}")]
    InvalidFormat(String),

    /// Missing required attribute
    #[error("Missing required attribute: {0}")]
    MissingAttribute(String),

    /// Missing required variable
    #[error("Missing required variable: {0}")]
    MissingVariable(String),

    /// Invalid sweep index
    #[error("Invalid sweep index: {0}")]
    InvalidSweepIndex(usize),

    /// Data conversion error
    #[error("Data conversion error: {0}")]
    Conversion(String),

    /// Malformed record at a specific byte offset
    #[error("Malformed record at offset {offset}: {msg}")]
    MalformedRecord {
        /// Byte offset within the source where the error was detected
        offset: u64,
        /// Diagnostic message
        msg: String,
    },

    /// Decode error from a downstream parser (e.g., the `nexrad` crate).
    #[error("Decode error: {0}")]
    Decode(String),

    /// The output encoding a caller requested is incompatible with what
    /// the source data declares — a moment's on-wire
    /// `word_size`/`scale`/`offset` cannot be represented exactly on the
    /// requested grid, or the requested output shape doesn't fit the
    /// data. Surfaces to Python as `radish.MomentEncodingError`.
    ///
    /// Deliberately distinct from [`RadishError::Decode`]: the bytes
    /// parsed fine, it's the *request* that can't be honoured, and
    /// silently approximating would put physically wrong values in the
    /// caller's array.
    #[error("Moment encoding error: {0}")]
    MomentEncoding(String),

    /// Unsupported feature
    #[error("Unsupported feature: {0}")]
    Unsupported(String),

    /// General error
    #[error("Error: {0}")]
    General(String),
}

impl From<String> for RadishError {
    fn from(s: String) -> Self {
        RadishError::General(s)
    }
}

impl From<&str> for RadishError {
    fn from(s: &str) -> Self {
        RadishError::General(s.to_string())
    }
}
