use std::collections::HashMap;

/// Represents a connected device with its attributes.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub attributes: HashMap<String, String>,
}

/// Request to publish an event to a specific client channel.
pub struct SendEventToClientRequest {
    pub client_id: String,
    pub event_name: String,
    pub event: Vec<u8>,
}

/// Request to publish an event to a specific device of a client.
pub struct SendEventToClientDeviceRequest {
    pub client_id: String,
    pub device_id: String,
    pub event_name: String,
    pub event: Vec<u8>,
}

/// Request to publish an event to a topic.
pub struct SendEventToTopicRequest {
    pub topic: String,
    pub event_name: String,
    pub event: Vec<u8>,
}

/// Request to get all active devices for a client.
pub struct GetActiveDevicesRequest {
    pub client_id: String,
}
