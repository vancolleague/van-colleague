//! Serves a Bluetooth GATT application using the callback programming model.
use std::{
    collections::{BTreeMap, HashMap},
    iter::Peekable,
    slice::Iter,
    sync::Arc,
    time::{Duration, Instant},
};

use bluer::{
    adv::Advertisement,
    gatt::local::{
        Application, Characteristic, CharacteristicRead, CharacteristicWrite,
        CharacteristicWriteMethod, Service,
    },
    Uuid,
};
use futures::FutureExt;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::Mutex,
    time::sleep,
};

use device::{Action, Device, DEVICE_GROUPS};

use crate::thread_sharing::*;

#[allow(dead_code)]
const MANUFACTURER_ID: u16 = 0x45F1;

const HUB_UUID: Uuid = Uuid::from_u128(0x0da6f72304f342818b4827c89c208284);
const REBOOT_UUID: Uuid = Uuid::from_u128(0xeab109d7537d48bd9ce6041208c42692);

pub async fn run_ble_server(advertising_uuid: Uuid, services: Vec<Service>, ble_name: String) {
    let session = bluer::Session::new().await.expect("bluer session issue");
    let adapter = session
        .default_adapter()
        .await
        .expect("session adapter issue");
    adapter.set_powered(true).await.unwrap();

    let mut manufacturer_data = BTreeMap::new();
    manufacturer_data.insert(MANUFACTURER_ID, vec![0x21, 0x22, 0x23, 0x24]);
    let le_advertisement = Advertisement {
        service_uuids: vec![advertising_uuid].into_iter().collect(),
        manufacturer_data: manufacturer_data.clone(),
        discoverable: Some(true),
        local_name: Some(ble_name),
        ..Default::default()
    };
    let adv_handle = adapter
        .advertise(le_advertisement)
        .await
        .expect("adapter advertiser issue");

    println!(
        "Serving GATT service on Bluetooth adapter {}",
        adapter.name()
    );
    let app = Application {
        services: services,
        ..Default::default()
    };

    let app_handle = adapter
        .serve_gatt_application(app)
        .await
        .expect("Gatt application seriving issue");

    loop {
        sleep(Duration::from_secs(1000)).await;
    }
}

pub fn hub_reboot_service(
    shared_ble_command: Arc<Mutex<SharedBLECommand>>,
    primary: bool,
) -> Service {
    let shared_ble_command_write = Arc::clone(&shared_ble_command);
    Service {
        uuid: HUB_UUID,
        primary,
        characteristics: vec![Characteristic {
            uuid: REBOOT_UUID,
            write: Some(CharacteristicWrite {
                write: true,
                write_without_response: true,
                method: CharacteristicWriteMethod::Fun(Box::new(move |new_value, _req| {
                    let shared_ble_command_write_clone = shared_ble_command_write.clone();
                    async move {
                        let text = match std::str::from_utf8(&new_value) {
                            Ok(t) => t,
                            Err(_) => {
                                return Ok(());
                            }
                        };
                        let target: usize = match text.chars().take(1).collect::<String>().parse() {
                            Ok(t) => t,
                            Err(_) => {
                                return Ok(());
                            }
                        };
                        {
                            let mut shared_ble_command_write_guard =
                                shared_ble_command_write_clone.lock().await;
                            *shared_ble_command_write_guard =
                                SharedBLECommand::Reboot { node_count: target };
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

/// this isn't generic, it only works for Set becaue of how targets/values are handled
pub fn generic_read_write_service(
    service_uuid: Uuid,
    char_uuid: Uuid,
    shared_ble_command: Arc<Mutex<SharedBLECommand>>,
    primary: bool,
) -> Service {
    let shared_ble_command_read = Arc::clone(&shared_ble_command);
    let shared_ble_command_write = Arc::clone(&shared_ble_command);
    Service {
        uuid: service_uuid,
        primary,
        characteristics: vec![Characteristic {
            uuid: char_uuid,
            read: Some(CharacteristicRead {
                read: true,
                fun: Box::new(move |_req| {
                    let shared_ble_command_read_clone = shared_ble_command_read.clone();
                    async move {
                        loop {
                            let mut shared_ble_command_read_guard =
                                shared_ble_command_read_clone.lock().await;
                            if *shared_ble_command_read_guard == SharedBLECommand::NoUpdate {
                                *shared_ble_command_read_guard = SharedBLECommand::TargetInquiry {
                                    device_uuid: service_uuid,
                                };
                                break;
                            }
                            sleep(Duration::from_millis(3)).await;
                        }
                        let response =
                            await_for_inquiry_response(shared_ble_command_read_clone).await;
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
                    let shared_ble_command_write_clone = shared_ble_command_write.clone();
                    async move {
                        let text = std::str::from_utf8(&new_value);
                        match std::str::from_utf8(&new_value) {
                            Ok(text) => {
                                let target: Option<usize> =
                                    match text.chars().take(1).collect::<String>().parse() {
                                        Ok(t) => Some(t),
                                        Err(e) => {
                                            eprintln!(
                                                "Bad CharicteristicWrite text received: {}",
                                                e
                                            );
                                            return Ok(());
                                        }
                                    };
                                loop {
                                    let mut shared_ble_command_write_guard =
                                        shared_ble_command_write_clone.lock().await;
                                    if *shared_ble_command_write_guard == SharedBLECommand::NoUpdate
                                    {
                                        *shared_ble_command_write_guard =
                                            SharedBLECommand::Command {
                                                device_uuid: service_uuid,
                                                action: Action::from_u128(
                                                    char_uuid.as_u128(),
                                                    target,
                                                )
                                                .unwrap(),
                                            };
                                        break;
                                    }
                                    sleep(Duration::from_millis(3)).await;
                                }
                            }
                            Err(e) => {
                                eprintln!("Bad CharicteristicWrite value received: {}", e);
                            }
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
    shared_ble_command: Arc<Mutex<SharedBLECommand>>,
    devices: Vec<Device>,
) -> Service {
    let set_uuid: Uuid = Action::Set(0).to_uuid();
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
                    let shared_ble_command_clone = shared_ble_command.clone();
                    let device_ids = devices
                        .iter()
                        .map(|d| (d.name.clone(), d.uuid.clone()))
                        .collect::<HashMap<String, Uuid>>();
                    let device_group_ids = DEVICE_GROUPS
                        .iter()
                        .map(|ds| (ds.name.to_string(), Uuid::from_u128(ds.uuid_number.clone())))
                        .collect::<HashMap<String, Uuid>>();
                    async move {
                        let command = match std::str::from_utf8(&new_value) {
                            Ok(t) => t.to_lowercase(),
                            Err(e) => {
                                eprintln!("Bad voice command received: {}", e);
                                return Ok(());
                            }
                        };
                        let command = command
                            .trim_end()
                            .trim_end_matches('\0')
                            .split_whitespace()
                            .collect::<Vec<&str>>();
                        let mut command_iter = command.iter().peekable();

                        // check if the device is in a group such as "lights" or "fans"
                        let mut device_id = get_device_ids(device_group_ids, &mut command_iter);
                        // if it's not, look at the hardware device names such as "kitchen light"
                        // or "roof vent"
                        if device_id.is_none() {
                            command_iter = command.iter().peekable();
                            device_id = get_device_ids(device_ids, &mut command_iter);
                        }
                        if device_id.is_none() {
                            eprintln!("Bad device name given");
                            return Ok(());
                        }
                        let (_name, uuid) = device_id.unwrap();

                        let action = match command_iter.next() {
                            Some(&"at") => "set", // for when it hears "at" instead of "set"
                            Some(a) => a,
                            None => {
                                return Ok(());
                            } //None => panic!("failed to get an action"), //return HttpResponse::Ok().body("Oops, we didn't get an action!"),
                        };
                        let target = match command_iter.next() {
                            Some(t) => {
                                if t.is_empty() {
                                    None
                                } else {
                                    let target: usize = match t {
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
                                        _ => {
                                            return Ok(());
                                        }
                                    };
                                    Some(target)
                                }
                            }
                            None => None,
                        };

                        let action = match Action::from_str(action, target) {
                            Ok(a) => a,
                            Err(_) => return Ok(()),
                        };
                        loop {
                            let mut shared_ble_command_guard =
                                shared_ble_command_clone.lock().await;
                            if *shared_ble_command_guard == SharedBLECommand::NoUpdate {
                                *shared_ble_command_guard = SharedBLECommand::Command {
                                    device_uuid: service_uuid,
                                    action: action.clone(),
                                };
                                break;
                            }
                            sleep(Duration::from_millis(3)).await;
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

async fn await_for_inquiry_response(shared_ble_action: Arc<Mutex<SharedBLECommand>>) -> usize {
    println!("Waiting???????????");
    //let start_time = Instant::now();
    //let timeout = 10;
    //   while start_time.elapsed().as_secs() < timeout {
    loop {
        {
            let mut lock = shared_ble_action.lock().await;
            match &*lock {
                SharedBLECommand::TargetResponse { ref target } => {
                    let thing = target.clone();
                    *lock = SharedBLECommand::NoUpdate;
                    return thing;
                }
                _ => {}
            }
        }
        //        sleep(Duration::from_millis(9)).await;
    }
    // 0
}

fn get_device_ids(
    group_names: HashMap<String, Uuid>,
    command_words: &mut Peekable<Iter<'_, &str>>,
) -> Option<(String, Uuid)> {
    let mut partial_device_name = match command_words.next() {
        Some(t) => t.to_string(),
        None => return None,
    };
    let mut next_command_word = command_words.peek();

    let mut found_device_name = "".to_string();
    while next_command_word.is_some() {
        if group_names.keys().any(|n| n == &partial_device_name) {
            //if device_names.contains(&&device_name.to_string()) {
            found_device_name = partial_device_name;
            break;
        }
        partial_device_name = format!(
            "{} {}",
            partial_device_name,
            next_command_word.expect("This value checked in while loop")
        );
        command_words.next();
        next_command_word = command_words.peek();
    }

    match group_names.get(&found_device_name) {
        Some(u) => Some((found_device_name, u.clone())),
        None => None,
    }
    /*
    if found_device_name == "".to_string() {
        return None;
    }

    for (n, u) in name_list {
        if n == found_device_name {
            return Some((u.clone(), found_device_name));
        }
    }

    None*/
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_await_for_inquiry_response() {
        let shared_ble_command = Arc::new(Mutex::new(SharedBLECommand::NoUpdate));
        let shared_ble_command_clone = shared_ble_command.clone();
        let shared_ble_command_clone2 = shared_ble_command.clone();
        tokio::spawn(async move {
            sleep(Duration::from_secs(1)).await;
            let mut shared_ble_command_guard = shared_ble_command_clone.lock().await;
            *shared_ble_command_guard = SharedBLECommand::TargetResponse { target: 5 };
        });
        let response = await_for_inquiry_response(shared_ble_command);
        assert_eq!(
            *shared_ble_command_clone2.lock().await,
            SharedBLECommand::NoUpdate
        );
        assert_eq!(response.await, 5);
    }

    /*#[tokio::test]
    async fn test_await_for_inquiry_response_no_response() {
        let shared_ble_command = Arc::new(Mutex::new(SharedBLECommand::NoUpdate));
        let response = await_for_inquiry_response(shared_ble_command);
        assert_eq!(response.await, 0);
    }*/

    #[test]
    fn test_get_device_ids_two_words() {
        let name_list = HashMap::from([
            ("lights".to_string(), Uuid::from_u128(0x0)),
            ("kitchen light".to_string(), Uuid::from_u128(0x1)),
            ("bedroom light".to_string(), Uuid::from_u128(0x2)),
            ("four".to_string(), Uuid::from_u128(0x3)),
        ]);

        let command_words = "kitchen light up 3".to_string();
        let command_words = command_words.to_lowercase();
        let command_words = command_words
            .trim_end()
            .trim_end_matches('\0')
            .split_whitespace()
            .collect::<Vec<&str>>();
        let mut command_words = command_words.iter().peekable();

        let name = get_device_ids(name_list, &mut command_words);

        assert_eq!(
            name,
            Some(("kitchen light".to_string(), Uuid::from_u128(0x1)))
        );
    }

    #[test]
    fn test_get_device_ids_one_word() {
        let name_list = HashMap::from([
            ("lights".to_string(), Uuid::from_u128(0x0)),
            ("kitchen light".to_string(), Uuid::from_u128(0x1)),
            ("bedroom light".to_string(), Uuid::from_u128(0x2)),
            ("four".to_string(), Uuid::from_u128(0x3)),
        ]);

        let command_words = "lights up 3".to_string();
        let command_words = command_words.to_lowercase();
        let command_words = command_words
            .trim_end()
            .trim_end_matches('\0')
            .split_whitespace()
            .collect::<Vec<&str>>();
        let mut command_words = command_words.iter().peekable();

        let name = get_device_ids(name_list, &mut command_words);

        assert_eq!(name, Some(("lights".to_string(), Uuid::from_u128(0x0))));
    }

    #[test]
    fn test_get_device_ids_none() {
        let name_list = HashMap::from([
            ("lights".to_string(), Uuid::from_u128(0x0)),
            ("kitchen light".to_string(), Uuid::from_u128(0x1)),
            ("bedroom light".to_string(), Uuid::from_u128(0x2)),
            ("four".to_string(), Uuid::from_u128(0x3)),
        ]);

        let command_words = "bathroom light up 3".to_string();
        let command_words = command_words.to_lowercase();
        let command_words = command_words
            .trim_end()
            .trim_end_matches('\0')
            .split_whitespace()
            .collect::<Vec<&str>>();
        let mut command_words = command_words.iter().peekable();

        let name = get_device_ids(name_list, &mut command_words);

        assert_eq!(name, None);
    }
}
