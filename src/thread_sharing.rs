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
    Restart { node_count: usize },
    NoUpdate,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq)]
pub enum SharedBLERead {
    Inquiry { device_uuid: Uuid },
    Response { target: usize },
    NoUpdate,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq)]
pub enum SharedBLEWrite {
    Command {
        device_uuid: Uuid,
        action: device::Action,
    },
    Response {
        message: u16,
    },
    NoUpdate,
}
