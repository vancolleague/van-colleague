use std::collections::HashMap;
use std::sync::Arc;

use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer};
use bluer::Uuid;
use tokio::{main, spawn, sync::Mutex};

use device::{Action, Device};
//mod crate::shared_request;
//use crate::devices::DEVICES;
use crate::thread_sharing::{SharedConfig, SharedGetRequest};

async fn index(
    req: HttpRequest,
    _shared_config_clone: web::Data<Arc<Mutex<SharedConfig>>>,
    shared_request_clone: web::Data<Arc<Mutex<SharedGetRequest>>>,
) -> &'static str {
    println!("REQ: {req:?}");
    {
        let mut shared_request = shared_request_clone.lock().await;
        *shared_request = SharedGetRequest::NoUpdate;
    }
    "Nothing here!"
}

// Expect to be http://ip:8080?device=device%20name&action=act&target=tar
async fn parsed_command(
    _req: HttpRequest,
    info: web::Query<HashMap<String, String>>,
    _shared_config_clone: web::Data<Arc<Mutex<SharedConfig>>>,
    shared_request_clone: web::Data<Arc<Mutex<SharedGetRequest>>>,
    _devices: web::Data<Vec<(String, Uuid)>>,
) -> HttpResponse {
    let uuid = match info.get("uuid") {
        Some(u) => match format!("0x{}", u.replace("-", "")).parse::<u128>() {
            Ok(numb) => Uuid::from_u128(numb),
            Err(_) => return HttpResponse::Ok().body("Bad Uuid given"),
        },
        None => return HttpResponse::Ok().body("Oops, we didn't get the Uuid"),
    };

    let target: Option<usize> = match info.get("target") {
        Some(t) => {
            if t.as_str() != "" {
                match t.parse::<usize>() {
                    Ok(n) => {
                        if n < 8 {
                            Some(n)
                        } else {
                            return HttpResponse::Ok().body("Oops, Target should be 0 <= t < 8");
                        }
                    }
                    Err(_) => return HttpResponse::Ok().body("Oops, Target should be 0 <= t < 8"),
                }
            } else {
                None
            }
        }
        None => None,
    };

    let action = match info.get("action") {
        Some(a) => a.to_lowercase(),
        None => return HttpResponse::Ok().body("Oops, we didn't get the Action"),
    };

    let action = match Action::from_str(action.as_str(), target) {
        Ok(a) => a,
        Err(_) => return HttpResponse::Ok().body("Action wasn't a valid action."),
    };

    let result = {
        let mut shared_request = shared_request_clone.lock().await;
        *shared_request = SharedGetRequest::Command {
            device_uuid: uuid,
            action: action,
        };
        serde_json::to_string(&(*shared_request)).unwrap()
    };
    HttpResponse::Ok().body(result)
}

async fn command(
    _req: HttpRequest,
    info: web::Query<HashMap<String, String>>,
    _shared_config_clone: web::Data<Arc<Mutex<SharedConfig>>>,
    shared_request_clone: web::Data<Arc<Mutex<SharedGetRequest>>>,
    devices: web::Data<Vec<(String, Uuid)>>,
) -> HttpResponse {
    let command = match info.get("command") {
        Some(i) => i,
        None => return HttpResponse::Ok().body("Oops, we didn't get the command"),
    };

    let mut device = String::new();
    let command = command.replace("%20", " ").to_lowercase();
    let mut command = command.split_whitespace();
    while devices
        .clone()
        .iter()
        .map(|(n, _)| n)
        .collect::<Vec<&String>>()
        .contains(&&device)
    {
        //while !DEVICES.contains_key(&device.as_str()) {
        let word = match command.next() {
            Some(w) => w,
            None => return HttpResponse::Ok().body("Oops, we didn't get a device!"),
        };

        if device.is_empty() {
            device = word.to_string();
        } else {
            device = format!("{} {}", &device, &word);
        }
    }

    let mut uuid = Uuid::from_u128(0x0);
    for (n, u) in devices.iter() {
        if &device == n {
            uuid = u.clone();
            break;
        }
    }
    if uuid.as_u128() == 0x0 {
        return HttpResponse::Ok().body("Couldn't get the uuid of the named device.");
    }

    let action = match command.next() {
        Some(a) => a,
        None => return HttpResponse::Ok().body("Oops, we didn't get an action!"),
    };

    let target = match command.next() {
        Some(t) => {
            if t.is_empty() {
                None
            } else {
                match t.parse::<usize>() {
                    Ok(n) => {
                        if n < 8 {
                            Some(n)
                        } else {
                            return HttpResponse::Ok().body("Oops, Target should be 0 <= t < 8");
                        }
                    }
                    Err(_) => {
                        return HttpResponse::Ok()
                            .body("Oops, Target should be a number, 0 though 7")
                    }
                }
            }
        }
        None => None,
    };

    let action = match Action::from_str(action, target) {
        Ok(a) => a,
        Err(_) => return HttpResponse::Ok().body("Action wasn't a valid action."),
    };

    let result = {
        let mut shared_request = shared_request_clone.lock().await;
        *shared_request = SharedGetRequest::Command {
            device_uuid: uuid,
            action: action,
        };
        serde_json::to_string(&(*shared_request)).unwrap()
    };
    HttpResponse::Ok().body(result)
}

pub async fn run_http_server(
    shared_config_clone: Arc<Mutex<SharedConfig>>,
    shared_request_clone: Arc<Mutex<SharedGetRequest>>,
    devices: Vec<(String, Uuid)>,
) -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    log::info!("starting HTTP server at http://localhost:8080");

    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default())
            .app_data(web::Data::new(shared_config_clone.clone()))
            .app_data(web::Data::new(shared_request_clone.clone()))
            .app_data(web::Data::new(devices.clone()))
            .service(web::resource("/").to(index))
            .service(web::resource("/parsed_command").to(parsed_command))
            .service(web::resource("/command").to(command))
    })
    .bind(("0.0.0.0", 8080))
    .unwrap()
    .run()
    .await
}
