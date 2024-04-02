use futures::future::join_all;
use std::collections::HashMap;
use std::process::Command;

use regex::Regex;

use bluer::Uuid;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use device::Device;

/// A struct to store a device along with the IP address where it's located
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct LocatedDevice {
    pub device: Device,
    pub ip: String,
}

/// Get all of the devices along with their locateions.
///
/// Returns a HashMap where the keys are device Uuids
/// and values are LocatedDevices
pub async fn get_devices() -> HashMap<Uuid, LocatedDevice> {
    let ips = get_ips();

    let mut devices: HashMap<Uuid, LocatedDevice> = HashMap::new();

    let located_devices_futures: Vec<_> = ips
        .into_iter()
        .map(|ip| tokio::spawn(get_node_devices(ip)))
        .collect();

    let located_devices_results: Vec<_> = join_all(located_devices_futures).await;

    for located_devices_result in located_devices_results {
        if located_devices_result.is_ok() {
            let located_devices_result = located_devices_result.expect("Ok was just checked!");
            if located_devices_result.is_ok() {
                let located_devices_result = located_devices_result.expect("Was just checked!");
                for located_device in located_devices_result {
                    devices.insert(located_device.device.uuid.clone(), located_device);
                }
            }
        }
    }
    devices
}

/// Gets the status of a device
///
/// - 'ip': the ip address of the node that the device is on
/// - 'uuid': the uuid of the device
pub async fn get_device_status(ip: &String, uuid: &Uuid) -> Result<Device, String> {
    let url = format!("http://{}/status?uuid={}", ip, uuid.to_string());
    let device_text = match reqwest::get(&url).await {
        Ok(response) => match response.text().await {
            Ok(maybe_device_text) => maybe_device_text,
            Err(_) => {
                eprintln!("No response or nothing in get_device_status");
                return Err("No response or nothing in get_device_status".to_string());
            }
        },
        Err(_) => {
            eprintln!("No response or something in get_device_status");
            return Err("No response or something in get_device_status".to_string());
        }
    };

    match Device::from_json(&device_text) {
        Ok(d) => Ok(d),
        _ => {
            eprintln!(
                "Oops, didn't get the device as expected, apparent IP or name issue"
            );
            Err("Oops, didn't get the device as expected, apparent IP or name issue".to_string())
        }
    }
}

//async fn get_node_devices(ip: String) -> Option<HashMap<Uuid, LocatedDevice>> {
async fn get_node_devices(ip: String) -> Result<Vec<LocatedDevice>, String> {
    let url = format!("http://{}/devices", ip);
    let devices_text = match reqwest::get(&url).await {
        Ok(response) => match response.text().await {
            Ok(maybe_devices_text) => maybe_devices_text,
            Err(_) => {
                eprintln!("Error with getting the text from reqwest response");
                return Err("Error with getting the text from reqwest response".to_string());
            }
        },
        Err(_) => {
            eprintln!("Error with reqwest get");
            return Err("Error with reqwest get".to_string());
        }
    };
    let devices_json: Value = match serde_json::from_str(devices_text.as_str()) {
        Ok(dj) => dj,
        Err(_) => {
            eprintln!("Error with parsing json");
            return Err("Error with parsing json".to_string());
        }
    };
    let mut located_devices = Vec::new();
    if devices_json.is_object() {
        for (_, value) in devices_json
            .as_object()
            .expect("Just tested that it's an object")
        {
            let device = match Device::from_json(&value.to_string()) {
                Ok(d) => d,
                Err(_) => {
                    eprintln!("Error parsing json");
                    return Err("Error parsing json".to_string());
                }
            };
            located_devices.push(LocatedDevice {
                ip: ip.clone(),
                device,
            });
            /*located_devices.insert(
                device.uuid.clone(),
                LocatedDevice {
                    device,
                    ip: ip.clone(),
                },
            );*/
        }
    }
    Ok(located_devices)
}

fn run_nmap() -> String {
    let output = Command::new("nmap")
        .arg("-sn")
        .arg("192.168.2.0/24")
        .output()
        .expect("Failed to execute nmap command, perhapse it needs to be installed.");

    if output.status.success() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        panic!(
            "Command failed: {}, perhapse nmap needs to be installed.",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn extract_ips(nmap_output: &str) -> Vec<String> {
    let ip_regex =
        Regex::new(r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b").expect("ip search/extraction failed.");
    ip_regex
        .find_iter(nmap_output)
        .map(|match_| match_.as_str().to_string())
        .collect()
}

fn get_ips() -> Vec<String> {
    let nmap_output = run_nmap();
    extract_ips(&nmap_output)
}
