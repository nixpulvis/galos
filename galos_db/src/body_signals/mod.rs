//! What is written on a body's surface, short of scanning it
//!
//! Geology, biology, and the rest of what can be landed on and dug up. Stored
//! apart from [`crate::bodies`] because a signal is not a property of a body
//! the way its mass or its orbit is, and because it arrives without one: the
//! honk finds signals on bodies nothing has yet identified, and a row here
//! routinely predates the row in `bodies` it describes.
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodySignal {
    pub system_address: i64,
    pub body_id: i16,
    /// What kind of signal, as the game names it, e.g.
    /// `$SAA_SignalType_Geological;`
    pub signal_type: String,
    pub count: i32,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

mod create;
mod fetch;
