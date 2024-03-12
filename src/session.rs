use std::collections::HashMap;
use std::default::Default;
use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bluer::{gatt::local::Service, Uuid};
use clap::Command;
use clap::{Arg, ArgMatches};
use fs2::FileExt;
use tokio::{
    sync::Mutex,
    time::Duration,
};

use crate::ble_server;
use crate::devices;
use crate::devices::{get_devices, LocatedDevice};
use crate::http_server;
use crate::thread_sharing::*;
use device::{Action, Device, DEVICE_GROUPS};

const RUN_COMMAND: &str = "run";
const SHUTDOWN_COMMAND: &str = "shutdown";
const STATUS_COMMAND: &str = "status";

pub struct Session {
    pub ble_stuff: Vec<i32>,
    pub listen_port: u16,
    pub shared_get_request: Arc<Mutex<SharedGetRequest>>,
    pub shared_ble_command: Arc<Mutex<SharedBLECommand>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            ble_stuff: Vec::new(),
            listen_port: 4000,
            shared_get_request: Arc::new(Mutex::new(SharedGetRequest::NoUpdate)),
            shared_ble_command: Arc::new(Mutex::new(SharedBLECommand::NoUpdate)),
        }
    }
}

impl Session {
    fn new() -> Self {
        Self {
            ble_stuff: vec![],
            listen_port: 4000,
            shared_get_request: Arc::new(Mutex::new(SharedGetRequest::NoUpdate)),
            shared_ble_command: Arc::new(Mutex::new(SharedBLECommand::NoUpdate)),
        }
    }

    pub async fn setup(&mut self) -> (ArgMatches, HashMap<Uuid, LocatedDevice>) {
        let cli_command = get_user_args();

        let located_devices = match cli_command.subcommand() {
            Some((RUN_COMMAND, sub_matches)) => self.get_located_devices(&sub_matches).await,
            _ => HashMap::new(),
        };

        (cli_command, located_devices)
    }

    pub async fn run(
        &mut self,
        advertising_uuid: Uuid,
        command: ArgMatches,
        mut located_devices: HashMap<Uuid, LocatedDevice>,
        services: Vec<Service>,
    ) {
        match command.subcommand() {
            Some((RUN_COMMAND, _sub_matches)) => {
                let shutdown_flag = Arc::new(AtomicBool::new(false));

                self.start_console_command_sharing(Arc::clone(&shutdown_flag));

                setup_lock_file();

                self.run_http_server(&located_devices);

                run_ble_server(advertising_uuid, services);

                let port = self.listen_port.to_string();
                tokio::spawn(async move {
                    tokio::signal::ctrl_c().await.unwrap();
                    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
                    stream.write_all(SHUTDOWN_COMMAND.as_bytes()).unwrap();
                    process::exit(0);
                });

                // business logic
                let mut last_action = (Uuid::from_u128(0x0), Action::On);
                while !shutdown_flag.load(Ordering::SeqCst) {
                    {
                        use SharedGetRequest as SGR;
                        let mut shared_request_lock = self.shared_get_request.lock().await;
                        match &*shared_request_lock {
                            SGR::Command {
                                ref device_uuid,
                                ref action,
                            } => {
                                if last_action != (device_uuid.clone(), action.clone()) {
                                    last_action = (device_uuid.clone(), action.clone());
                                    let located_device = located_devices.get(&device_uuid);
                                    let _target = match action.get_value() {
                                        Some(t) => t.to_string(),
                                        None => "".to_string(),
                                    };
                                    match located_device {
                                        Some(d) => {
                                            let _ = update_device(&d.ip, &device_uuid, &action);
                                            *shared_request_lock = SharedGetRequest::NoUpdate;
                                        }
                                        None => {
                                            //println!("no device found");
                                        }
                                    }
                                }
                            }
                            SGR::NoUpdate => {}
                        }
                    }
                    {
                        use SharedBLECommand as SBC;
                        let mut shared_command_lock = self.shared_ble_command.lock().await;
                        match &*shared_command_lock {
                            SBC::Command {
                                ref device_uuid,
                                ref action,
                            } => {
                                let mut already_processed = false;
                                for device_group in DEVICE_GROUPS.iter() {
                                    if device_uuid != &Uuid::from_u128(device_group.uuid_number.clone()) {
                                        continue;
                                    }
                                    for (device_uuid, located_device) in located_devices.iter_mut() {
                                        if located_device.device.device_group == Some(device_group.device_group) {
                                            // TODO: seems like updates could be made in parallel
                                            update_device(&located_device.ip, &device_uuid, &action).await;
                                        }
                                    }
                                    already_processed = true;
                                    break;
                                }
                                if !already_processed {
                                    let located_device =
                                        located_devices.get_mut(&device_uuid).unwrap();
                                    update_device(&located_device.ip, &device_uuid, &action).await;
                                }
                                *shared_command_lock = SBC::NoUpdate;
                            }
                            SBC::TargetInquiry { ref device_uuid } => {
                                let located_device = located_devices.get(&device_uuid).unwrap();
                                let device = get_device_status_helper(
                                    located_device.ip.clone(),
                                    device_uuid.clone(),
                                )
                                .await;
                                println!("response: {:?}, {:?}", &device_uuid, &device);
                                *shared_command_lock = SBC::TargetResponse {
                                    target: device.unwrap().get_target(),
                                };
                            }
                            SBC::TargetResponse { .. } => {}
                            SBC::NoUpdate => {}
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }

                println!("Shutdown!!!!!!!!");
                process::exit(0);
            }
            Some((SHUTDOWN_COMMAND, _sub_matches)) => {
                println!("Shutting down the program!!!");
                let mut stream =
                    TcpStream::connect(format!("127.0.0.1:{}", self.listen_port.to_string()))
                        .unwrap();
                stream.write_all(SHUTDOWN_COMMAND.as_bytes()).unwrap();
                process::exit(0);
            }
            Some((STATUS_COMMAND, _sub_matches)) => {
                println!("Getting status...");
                match TcpStream::connect(format!("127.0.0.1:{}", self.listen_port.to_string())) {
                    Ok(mut stream) => {
                        println!("    Running");
                        stream.write_all(STATUS_COMMAND.as_bytes()).unwrap();
                        process::exit(0);
                    }
                    Err(e) => {
                        println!("    Not running");
                    }
                }
            }
            _ => {
                println!("You must enter a command, perhapse you wanted:");
                println!("  > hub run");
                println!("or");
                println!("  > hub help");
                process::exit(1);
            }
        }
    }

    /// Spawn a thread to handle the TCP server that's userd for sending/receiving commands between
    /// console windows
    fn start_console_command_sharing(&self, shutdown_flag: Arc<AtomicBool>) {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.listen_port.to_string()))
            .expect("Failed to bind to address");
        tokio::spawn(async move {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let shutdown_flag_clone = Arc::clone(&shutdown_flag);
                        handle_client(stream, shutdown_flag_clone).await;
                    }
                    Err(e) => eprintln!("Connection failed: {}", e),
                }
            }
        });
    }

    /// Get the list of connected devices if applicable
    async fn get_located_devices(&self, sub_matches: &&ArgMatches) -> HashMap<Uuid, LocatedDevice> {
        let mut located_devices = HashMap::new();
        let node_count: Option<&String> = sub_matches.get_one("node-count");
        if sub_matches.get_flag("no-nodes") {
            println!("Skipping getting devices!");
        } else {
            println!("Getting Devices!!");
            match node_count {
                Some(nc) => {
                    let nc: usize = nc.parse().unwrap();
                    let mut i = 0;
                    while located_devices.len() < nc && i < 10 {
                        std::thread::sleep(Duration::from_millis(10000));
                        located_devices = get_devices().await;
                        i += 1;
                    }
                }
                None => {
                    std::thread::sleep(Duration::from_millis(10000));
                    located_devices = get_devices().await;
                }
            }
        }

        println!("Devices:");
        for device in located_devices.values() {
            println!("    {}, {}", &device.device.uuid, &device.device.name);
        }

        located_devices
    }

    fn run_http_server(&self, located_devices: &HashMap<Uuid, LocatedDevice>) {
        // Start the http server with the appropreate info passed in
        let shared_config = Arc::new(Mutex::new(SharedConfig {
            verbosity: "none".to_string(),
        }));
        let shared_config_clone = shared_config.clone();
        let shared_request_clone = Arc::clone(&self.shared_get_request);
        let devices = located_devices
            .iter()
            .map(|(u, ld)| (ld.device.name.clone(), u.clone()))
            .collect::<Vec<(String, Uuid)>>();
        tokio::spawn(async move {
            http_server::run_http_server(shared_config_clone, shared_request_clone, devices).await
        });
        println!("Http server started");
    }
}

fn run_ble_server(
    advertising_uuid: Uuid,
    services: Vec<Service>,
) {
    tokio::spawn(async move { ble_server::run_ble_server(advertising_uuid, services).await });
}

async fn update_device(ip: &String, uuid: &Uuid, action: &Action) {
    let target = match action.get_value() {
        Some(t) => t.to_string(),
        None => "".to_string(),
    };

    let url = format!(
        "http://{}/command?uuid={}&action={}&target={}",
        &ip,
        &uuid.to_string(),
        &action.to_str().to_string(),
        &target,
    );
    println!("{}", &url);
    reqwest::get(&url).await.unwrap();
}

/// Needed so that the ip and uuid are owned and thus not dropped
async fn get_device_status_helper(ip: String, uuid: Uuid) -> Result<Device, String> {
    devices::get_device_status(&ip, &uuid).await
}

async fn handle_client(mut stream: TcpStream, shutdown_flag: Arc<AtomicBool>) {
    let mut buffer = [0; 1024];
    match stream.read(&mut buffer) {
        Ok(size) => {
            let received = String::from_utf8_lossy(&buffer[..size]);
            let received = received.trim();
            match received {
                SHUTDOWN_COMMAND => {
                    shutdown_flag.store(true, Ordering::SeqCst);
                }
                STATUS_COMMAND => {
                    if check_if_lock_file_exists() {
                        println!("Server runnign!");
                    } else {
                        println!("Server not runnign!");
                    }
                }
                _ => {}
            }
        }
        Err(e) => eprintln!("Failed to receive data: {}", e),
    }
}

fn setup_lock_file() {
    // Get the current working directory
    let current_dir = match env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Failed to determine current directory: {}", e);
            process::exit(1);
        }
    };

    // Construct the path to the lock file
    let lock_path = current_dir.join("hub_app.lock");
    let file = match File::create(&lock_path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("Failed to create lock file: {}", e);
            process::exit(1);
        }
    };

    // Try to acquire an exclusive lock
    if file.try_lock_exclusive().is_err() {
        eprintln!("Another instance of the application is already running.");
        process::exit(1);
    }
}

fn check_if_lock_file_exists() -> bool {
    // Get the current working directory
    let current_dir = match env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Failed to determine current directory: {}", e);
            process::exit(1);
        }
    };
    
    // Construct the path to the lock file
    let lock_path = current_dir.join("hub_app.lock");

    lock_path.as_path().exists()
}

pub fn get_user_args() -> ArgMatches {
    Command::new("Hub")
        .version("0.1")
        .author("Chad DeRosier, <chad.derosier@tutanota.com>")
        .about("Runs van automation stuff.")
        .subcommand(
            Command::new("run")
                .about("Runs the application") // ... additional settings or arguments specific to "run" ...
                .arg(
                    Arg::new("no-nodes")
                        .short('n')
                        .long("no-nodes")
                        .action(clap::ArgAction::SetTrue)
                        .help("Run without waiting to find all of the expected nodes"),
                )
                .arg(
                    Arg::new("node-count")
                        .short('c')
                        .long("node-count")
                        .action(clap::ArgAction::Set)
                        .help("Set the number of nodes to look for."),
                ),
        )
        .subcommand(Command::new("shutdown").about("Shutdown's the program and it's it all down"))
        .subcommand(Command::new("status").about("Shows some basic status info"))
        .arg(
            Arg::new("log_level")
                .long("log-level")
                .value_name("LEVEL")
                .help("Sets the level of logging"),
        )
        .arg(
            Arg::new("quiet")
                .short('q')
                .long("quiet")
                .help("Silences most output"),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Increases verbosity of output"),
        )
        .get_matches()
}
