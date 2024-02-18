//! Serves a Bluetooth GATT application using the callback programming model.
use std::{
    collections::{BTreeMap, HashMap},
    iter::Peekable,
    slice::Iter,
    sync::Arc,
    time::Duration,
};

use bluer::{
    adv::Advertisement,
    gatt::local::{
        Application, Characteristic,
        CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, Service,
    },
    Uuid,
};
use futures::FutureExt;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::Mutex,
    time::sleep,
};

use device::{Action, DEVICE_TYPES};

use crate::devices::LocatedDevice;
use crate::thread_sharing::*;

#[allow(dead_code)]
const MANUFACTURER_ID: u16 = 0x45F1;

pub async fn run_ble_server(advertising_uuid: Uuid, services: Vec<Service>) {
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
                fun: Box::new(move |_req| {
                    let shared_command_read_clone = shared_command_read.clone();
                    async move {
                        {
                            let mut shared_command_read_guard =
                                shared_command_read_clone.lock().await;
                            *shared_command_read_guard = SharedBLECommand::TargetInquiry {
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
                method: CharacteristicWriteMethod::Fun(Box::new(move |new_value, _req| {
                    let shared_command_write_clone = shared_command_write.clone();
                    async move {
                        let text = std::str::from_utf8(&new_value).unwrap();
                        let target: usize =
                            text.chars().take(1).collect::<String>().parse().unwrap();
                        {
                            let mut shared_command_write_guard =
                                shared_command_write_clone.lock().await;
                            *shared_command_write_guard = SharedBLECommand::Command {
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
    Service {
        uuid: service_uuid,
        primary: true,
        characteristics: vec![Characteristic {
            uuid: set_uuid,
            write: Some(CharacteristicWrite {
                write: true,
                write_without_response: true,
                method: CharacteristicWriteMethod::Fun(Box::new(move |new_value, _req| {
                    println!("voice received");
                    let shared_command_clone = Arc::clone(&shared_command);
                    let located_devices_clone = located_devices
                        .iter()
                        .map(|(u, ld)| (ld.device.name.clone(), u.clone()))
                        .collect::<Vec<(String, Uuid)>>();
                    let device_types_clone = DEVICE_TYPES
                        .iter()
                        .map(|(_, n, u)| (n.to_string(), Uuid::from_u128(u.clone())))
                        .collect::<Vec<(String, Uuid)>>();
                    async move {
                        let command = std::str::from_utf8(&new_value).unwrap();
                        let command = command.to_lowercase();
                        let command = command.trim_end();
                        let command = command.trim_end_matches('\0');
                        let command = command.split_whitespace().collect::<Vec<&str>>();
                        let mut command_iter = command.iter().peekable();

                        let mut device_uuid =
                            get_device_name(device_types_clone, &mut command_iter);
                        if device_uuid.is_none() {
                            command_iter = command.iter().peekable();
                            device_uuid = get_device_name(located_devices_clone, &mut command_iter);
                        }
                        if device_uuid.is_none() {
                            panic!("Didn't get a device");
                        }
                        let (_device, uuid) = device_uuid.unwrap();

                        if uuid.as_u128() == 0x0 {
                            panic!("didn't get the device id");
                        }
                        let action = match command_iter.next() {
                            Some(&"at") => "set",
                            Some(a) => a,
                            None => panic!("failed to get an action"), //return HttpResponse::Ok().body("Oops, we didn't get an action!"),
                        };
                        let target = match command_iter.next() {
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

fn get_device_name(
    name_list: Vec<(String, Uuid)>,
    command_words: &mut Peekable<Iter<'_, &str>>,
) -> Option<(String, Uuid)> {
    let mut device_name = command_words.next().unwrap().to_string();
    let mut next_word = command_words.peek();

    let mut names = Vec::new();
    let mut uuids = Vec::new();
    for (n, u) in name_list.iter() {
        names.push(n);
        uuids.push(u);
    }

    let mut found_name = "".to_string();
    while next_word.is_some() {
        if names.contains(&&device_name.to_string()) {
            found_name = device_name.to_string();
            break;
        }
        device_name = format!("{} {}", device_name, next_word.unwrap());
        command_words.next();
        next_word = command_words.peek();
    }

    if found_name == "".to_string() {
        return None;
    }

    for (n, u) in name_list {
        if n == found_name {
            return Some((found_name, u.clone()));
        }
    }

    None
}
