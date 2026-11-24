//! OS shell. Raw handles and unsafe operations are confined to this module.
//!
//! The first increment intentionally refuses to acquire a DRM master: connector selection,
//! VT/session ownership and the full restore/page-flip protocol are not yet implemented. A
//! caller must receive an explicit error rather than a false-success presentation backend.

use crate::PresentError;

/// Opaque placeholder for an acquired KMS session. No constructor currently yields one.
pub struct KmsDisplay { _private: () }

/// Open a direct-display session.
///
/// This path currently refuses before opening a device. Opening a primary DRM node can claim
/// display-master authority and disrupt the active console; until active-session detection,
/// state capture, page-flip completion, and guaranteed restoration are implemented together,
/// refusing is the only honest and safe behavior.
pub fn open_kms(_device: &str) -> Result<KmsDisplay, PresentError> {
    Err(PresentError::NotActive)
}
