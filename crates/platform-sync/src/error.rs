use platform_objects::ObjectError;
use thiserror::Error;

/// Why a sync could not finish. Nothing a sync stored before the error is
/// undone: positions only move once what they cover is stored, so the next
/// sync carries on from there.
#[derive(Debug, Error)]
pub enum SyncError {
    /// The device was revoked or its member left the workspace
    /// (`auth.device_wipe_required`): the app deletes the workspace's data.
    #[error("the device must wipe this workspace")]
    WipeRequired,
    /// The access token was refused: the app refreshes it and syncs again.
    #[error("the access token was refused")]
    Unauthorized,
    /// The stored pull position is older than the server keeps changes
    /// (`changes.position_expired`). The client resynchronises on its own;
    /// a transport returns this so it can.
    #[error("the pull position has expired")]
    PositionExpired,
    /// A background sync ran out of the time the OS allows. What it stored
    /// stays stored, and the next sync carries on from there.
    #[error("the sync ran out of time")]
    OutOfTime,
    /// The server answered with a problem the loop does not handle.
    #[error("the server answered {status} {code}")]
    Server { status: u16, code: String },
    /// The server could not be reached, or the request timed out.
    #[error("the server could not be reached: {0}")]
    Transport(String),
    /// The server's answer was not what its API describes.
    #[error("unexpected answer from the server: {0}")]
    Protocol(String),
    /// The device's engine refused a change.
    #[error("the replica refused a change: {0}")]
    Engine(#[from] ObjectError),
}

impl SyncError {
    pub(crate) fn protocol(detail: impl Into<String>) -> Self {
        Self::Protocol(detail.into())
    }
}
