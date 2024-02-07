//! Serves a Bluetooth GATT application using the callback programming model.
use std::str::FromStr;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

use bluer::{
    adv::Advertisement,
    gatt::local::{
        Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
        CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, Service,
    },
    Uuid,
};
use futures::FutureExt;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{mpsc, Mutex},
    time::sleep,
};

use device::{Action, Device, DeviceType};

use crate::devices::LocatedDevice;
use crate::thread_sharing::*;

#[allow(dead_code)]
const MANUFACTURER_ID: u16 = 0x45F1;

pub async fn run_ble_server(
    //    shared_action: Arc<Mutex<SharedBLECommand>>,
    advertising_uuid: Uuid,
    services: Vec<Service>,
) {
    let session = bluer::Session::new().await.unwrap();
    let adapter = session.default_adapter().await.unwrap();
    adapter.set_powered(true).await.unwrap();

    println!(
        "Advertising on Bluetooth adapter {} with address {}",
        adapter.name(),
        adapter.address().await.unwrap()
    );
    let mut manufacturer_data = BTreeMap::new();
    manufacturer_data.insert(MANUFACTURER_ID, vec![0x21, 0x22, 0x23, 0x24]);
    let le_advertisement = Advertisement {
        service_uuids: vec![advertising_uuid].into_iter().collect(),
        manufacturer_data: manufacturer_data.clone(),
        discoverable: Some(true),
        local_name: Some("VanColleague".to_string()),
        ..Default::default()
    };
    let adv_handle = adapter.advertise(le_advertisement).await.unwrap();

    println!(
        "Serving GATT service on Bluetooth adapter {}",
        adapter.name()
    );
    let app = Application {
        services: services,
        ..Default::default()
    };

    let app_handle = adapter.serve_gatt_application(app).await.unwrap();

    println!("Service ready. Press enter to quit.");
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let _ = lines.next_line().await;

    println!("Removing service and advertisement");
    drop(app_handle);
    drop(adv_handle);
    sleep(Duration::from_secs(1)).await;
}

pub fn slider_service(service_uuid: Uuid, shared_command: Arc<Mutex<SharedBLECommand>>) -> Service {
    let shared_command_read = Arc::clone(&shared_command);
    let shared_command_write = Arc::clone(&shared_command);
    let set_uuid: Uuid = Action::Set { target: 0 }.to_uuid();
    Service {
        uuid: service_uuid,
        primary: true,
        characteristics: vec![Characteristic {
            uuid: set_uuid,
            read: Some(CharacteristicRead {
                read: true,
                fun: Box::new(move |req| {
                    let shared_command_read_clone = shared_command_read.clone();
                    async move {
                        {
                            let mut shared_command_read_guarg =
                                shared_command_read_clone.lock().await;
                            *shared_command_read_guarg = SharedBLECommand::TargetInquiry {
                                device_uuid: service_uuid,
                            };
                        }
                        let response = await_for_inquiry_response(shared_command_read_clone).await;
                        println!("BLE response: {}", &response);
                        Ok(response.to_string().as_bytes().to_vec())
                    }
                    .boxed()
                }),
                ..Default::default()
            }),
            write: Some(CharacteristicWrite {
                write: true,
                write_without_response: true,
                method: CharacteristicWriteMethod::Fun(Box::new(move |new_value, req| {
                    let shared_command_write_clone = shared_command_write.clone();
                    async move {
                        let text = std::str::from_utf8(&new_value).unwrap();
                        let target: usize =
                            text.chars().take(1).collect::<String>().parse().unwrap();
                        {
                            let mut shared_command_write_guarg =
                                shared_command_write_clone.lock().await;
                            *shared_command_write_guarg = SharedBLECommand::Command {
                                device_uuid: service_uuid,
                                action: Action::Set { target: target },
                            };
                        }
                        Ok(())
                    }
                    .boxed()
                })),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}

pub fn voice_service(
    service_uuid: Uuid,
    shared_command: Arc<Mutex<SharedBLECommand>>,
    located_devices: HashMap<Uuid, LocatedDevice>,
) -> Service {
    let set_uuid: Uuid = Action::Set { target: 0 }.to_uuid();
    let devices = located_devices
        .iter()
        .map(|(u, ld)| (ld.device.name.clone(), u.clone()))
        .collect::<Vec<(String, Uuid)>>();
    Service {
        uuid: service_uuid,
        primary: true,
        characteristics: vec![Characteristic {
            uuid: set_uuid,
            write: Some(CharacteristicWrite {
                write: true,
                write_without_response: true,
                method: CharacteristicWriteMethod::Fun(Box::new(move |new_value, req| {
                    println!("voice recieved");
                    let shared_command_clone = Arc::clone(&shared_command);
                    let devices_clone = devices.clone();
                    async move {
                        let command = std::str::from_utf8(&new_value).unwrap();
                        let command = command.to_lowercase();
                        let command = command.trim_end();
                        let command = command.trim_end_matches('\0');
                        let command = command.split_whitespace().collect::<Vec<&str>>();
                        let mut command = command.iter();
                        let mut device = String::new();

                        while !devices_clone
                            .clone()
                            .iter()
                            .map(|(n, _)| n)
                            .collect::<Vec<&String>>()
                            .contains(&&device)
                        {
                            let word = match command.next() {
                                Some(w) => w,
                                None => {
                                    panic!("Didn't get the device name");
                                }
                            };

                            if device.is_empty() {
                                device = word.to_string();
                            } else {
                                device = format!("{} {}", &device, &word);
                            }
                        }
                        let mut uuid = Uuid::from_u128(0x0);
                        for (n, u) in devices_clone.iter() {
                            if &device == n {
                                uuid = u.clone();
                                break;
                            }
                        }
                        if uuid.as_u128() == 0x0 {
                            panic!("didn't get the device id");
                        }

                        let action = match command.next() {
                            Some(&"at") => "set",
                            Some(a) => a,
                            None => panic!("failed to get an action"), //return HttpResponse::Ok().body("Oops, we didn't get an action!"),
                        };
                        let target = match command.next() {
                            Some(t) => {
                                if t.is_empty() {
                                    None
                                } else {
                                    let t: usize = match t {
                                        &"zero" => 0,
                                        &"0" => 0,
                                        &"one" => 1,
                                        &"1" => 1,
                                        &"1:00" => 1,
                                        &"two" => 2,
                                        &"too" => 2,
                                        &"to" => 2,
                                        &"2" => 2,
                                        &"2:00" => 2,
                                        &"three" => 3,
                                        &"3" => 3,
                                        &"3:00" => 3,
                                        &"four" => 4,
                                        &"for" => 4,
                                        &"4" => 4,
                                        &"4:00" => 4,
                                        &"five" => 5,
                                        &"5" => 5,
                                        &"5:00" => 5,
                                        &"six" => 6,
                                        &"6" => 6,
                                        &"6:00" => 6,
                                        &"seven" => 7,
                                        &"7" => 7,
                                        &"7:00" => 7,
                                        _ => panic!("Bad target spoken"),
                                    };
                                    Some(t)
                                }
                            }
                            None => None,
                        };

                        let action = match Action::from_str(action, target) {
                            Ok(a) => a,
                            Err(_) => {
                                panic!("Issue creating the action");
                            }
                        };

                        {
                            let mut shared_command_guard = shared_command_clone.lock().await;
                            *shared_command_guard = SharedBLECommand::Command {
                                device_uuid: uuid,
                                action: action,
                            };
                        }
                        Ok(())
                    }
                    .boxed()
                })),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}

async fn await_for_inquiry_response(shared_action: Arc<Mutex<SharedBLECommand>>) -> usize {
    println!("Waiting???????????");
    loop {
        {
            let mut lock = shared_action.lock().await;
            match &*lock {
                SharedBLECommand::TargetResponse { ref target } => {
                    let thing = target.clone();
                    *lock = SharedBLECommand::NoUpdate;
                    return thing;
                }
                _ => {}
            }
        }
    }
}
