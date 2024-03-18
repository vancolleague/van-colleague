use bluer::Uuid;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SharedConfig {
    pub verbosity: String,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq)]
pub enum SharedGetRequest {
    Command {
        device_uuid: Uuid,
        action: device::Action,
    },
    NoUpdate,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq)]
pub enum SharedBLECommand {
    Command {
        device_uuid: Uuid,
        action: device::Action,
    },
    Reboot {
        node_count: usize,
    },
    TargetInquiry {
        device_uuid: Uuid,
    },
    TargetResponse {
        target: usize,
    },
    NoUpdate,
}
