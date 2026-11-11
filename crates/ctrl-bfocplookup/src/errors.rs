//! Port of internal/controller/bfocplookup/errors.go

use std::fmt;

/// RegistryAccessError indicates the operator failed to access the container registry
/// This is a transient error - the registry might be temporarily unavailable
#[derive(Debug)]
pub struct RegistryAccessError {
    pub repository: String,
    pub err: anyhow::Error,
}

impl fmt::Display for RegistryAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to access registry repository {}: {}",
            self.repository, self.err
        )
    }
}

impl std::error::Error for RegistryAccessError {
    /// `Unwrap()`
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.err.as_ref())
    }
}

/// RegistryAuthError indicates the operator lacks credentials to access the registry
/// This is a permanent error - user must provide valid credentials
#[derive(Debug)]
pub struct RegistryAuthError {
    pub repository: String,
    pub err: anyhow::Error,
}

impl fmt::Display for RegistryAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "authentication failed for registry repository {}: {}",
            self.repository, self.err
        )
    }
}

impl std::error::Error for RegistryAuthError {
    /// `Unwrap()`
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.err.as_ref())
    }
}

/// VersionNotFoundError indicates the OCP version tag was not found in the registry
/// This is a permanent error - the BlueField OCP layer image for this version hasn't been pushed yet
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionNotFoundError {
    pub version: String,
    pub repository: String,
}

impl fmt::Display for VersionNotFoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BlueField OCP layer image not found for OCP version {} in repository {}",
            self.version, self.repository
        )
    }
}

impl std::error::Error for VersionNotFoundError {}

/// InvalidImageFormatError indicates the ocpReleaseImage URL is malformed
/// This is a permanent error - user must fix CR spec
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidImageFormatError {
    pub message: String,
    pub url: String,
}

impl fmt::Display for InvalidImageFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid ocpReleaseImage format: {} (URL: {})",
            self.message, self.url
        )
    }
}

impl std::error::Error for InvalidImageFormatError {}

/// InvalidBlueFieldOCPLayerImageURLError indicates the constructed BlueField OCP layer image reference is invalid
/// This is a permanent error
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidBlueFieldOCPLayerImageURLError {
    pub version: String,
    pub url: String,
    pub message: String,
}

impl fmt::Display for InvalidBlueFieldOCPLayerImageURLError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BlueField OCP layer image URL is invalid for OCP version {}: {} (URL: {})",
            self.version, self.message, self.url
        )
    }
}

impl std::error::Error for InvalidBlueFieldOCPLayerImageURLError {}
