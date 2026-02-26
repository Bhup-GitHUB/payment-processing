use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub attributes: HashMap<String, String>,
}

pub struct SendEventToClientRequest {
    pub client_id: String,
    pub event_name: String,
    pub event: Vec<u8>,
}

#[allow(dead_code)]
pub struct SendEventToClientDeviceRequest {
    pub client_id: String,
    pub device_id: String,
    pub event_name: String,
    pub event: Vec<u8>,
}

#[allow(dead_code)]
pub struct SendEventToTopicRequest {
    pub topic: String,
    pub event_name: String,
    pub event: Vec<u8>,
}

pub struct GetActiveDevicesRequest {
    pub client_id: String,
}
