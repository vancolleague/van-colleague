use bluer::Uuid;
use serde::{Deserialize, Serialize};

use device;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SharedConfig {
    pub verbosity: String,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
pub enum SharedGetRequest {
    Command {
        device_uuid: Uuid,
        action: device::Action,
    },
    NoUpdate,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
pub enum SharedBLECommand {
    Command {
        device_uuid: Uuid,
        action: device::Action,
    },
    TargetInquiry {
        device_uuid: Uuid,
    },
    TargetResponse {
        target: usize,
    },
    NoUpdate,
}
