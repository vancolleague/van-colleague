use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bluer::{gatt::local::Service, Uuid};
use clap::Command as ClapCommand;
use clap::{Arg, ArgMatches};
use fs2::FileExt;
use tokio::{sync::Mutex, time::Duration};

use crate::ble_server;
use crate::cli_command::CLICommand;
use crate::devices::{self, LocatedDevice};
use crate::http_server;
use crate::thread_sharing::{SharedBLECommand, SharedConfig, SharedGetRequest};
use device::{Action, Device, DEVICE_GROUPS};

const VOICE_UUID: Uuid = Uuid::from_u128(0x7e1be1ebf9844e17b0f1049e02a39567);

const LOCK_FILE_NAME: &'static str = "hub_app.lock";

pub struct Session {
    pub ble_name: String,
    pub listen_port: u16,
    pub reboot_wait: usize,
    pub shared_get_request: Arc<Mutex<SharedGetRequest>>,
    pub shared_ble_command: Arc<Mutex<SharedBLECommand>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            ble_name: "VanColleague".to_string(),
            listen_port: 4000,
            reboot_wait: 10,
            shared_get_request: Arc::new(Mutex::new(SharedGetRequest::NoUpdate)),
            shared_ble_command: Arc::new(Mutex::new(SharedBLECommand::NoUpdate)),
        }
    }
}

impl Session {
    fn new(ble_name: String, listen_port: u16, reboot_wait: usize) -> Self {
        Self {
            ble_name,
            listen_port,
            reboot_wait,
            ..Default::default() /*ble_name: "VanColleague".to_string(),
                                 listen_port: 4000,
                                 shared_get_request: Arc::new(Mutex::new(SharedGetRequest::NoUpdate)),
                                 shared_ble_command: Arc::new(Mutex::new(SharedBLECommand::NoUpdate)),*/
        }
    }

    pub async fn run(&mut self) {
        let cli_command = get_user_args().get_matches();

        let sub_matches;
        let parsed_cli_command = match cli_command.subcommand() {
            Some((command, sc)) => {
                sub_matches = sc;
                match CLICommand::from_str(command) {
                    Ok(c) => c,
                    Err(_) => {
                        eprintln!("You must enter a command, perhapse you wanted:");
                        eprintln!("  > hub run");
                        eprintln!("or");
                        eprintln!("  > hub help");
                        process::exit(1);
                    }
                }
            }
            _ => {
                eprintln!("You must enter a command, perhapse you wanted:");
                eprintln!("  > hub run");
                eprintln!("or");
                eprintln!("  > hub help");
                process::exit(1);
            }
        };
        match parsed_cli_command {
            CLICommand::Run => {
                wait_to_start(&sub_matches);

                let shutdown_flag = Arc::new(AtomicBool::new(false));

                let test_connection =
                    TcpStream::connect(format!("127.0.0.1:{}", self.listen_port.to_string()));
                match test_connection {
                    Ok(_) => {
                        println!("Another instance is already running");
                        process::exit(0);
                    }
                    Err(_) => {}
                }
                //setup_lock_file();

                let node_count_text: Option<&String> = sub_matches.get_one("node-count");
                let node_count: Option<usize> = match node_count_text {
                    Some(count) => {
                        match count.parse() {
                            Ok(nc) => Some(nc),
                            Err(_) => {
                                eprintln!("An invalid -node-count was entered, it must be a posative integer");
                                process::exit(1);
                            }
                        }
                    }
                    None => None,
                };
                let mut located_devices = get_located_devices(node_count).await;
                // TODO: check for bad stuff here
                self.run_http_server(&located_devices);

                self.start_console_command_sharing(shutdown_flag.clone());

                let devices = located_devices
                    .values()
                    .map(|ld| ld.device.clone())
                    .collect::<Vec<Device>>();
                let services = get_ble_services(devices, self.shared_ble_command.clone());
                self.run_ble_server(VOICE_UUID, services);

                /// await a ctl-c command in a spawned thread while the main continues, and one received, exit
                let port = self.listen_port.to_string();
                tokio::spawn(async move {
                    tokio::signal::ctrl_c().await.unwrap();
                    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).expect("Had an issue connecting the ctrl-c watcher to the thread sharing TcpStream");
                    stream
                        .write_all(CLICommand::Shutdown.to_str().as_bytes())
                        .expect(
                            "Had an issue writing the ctrl-c shutdown command to the TcpStream",
                        );
                    process::exit(0);
                });

                // business logic
                let mut last_action = (Uuid::from_u128(0x0), Action::On);
                while !shutdown_flag.load(Ordering::SeqCst) {
                    {
                        use SharedGetRequest as SGR;
                        let mut shared_get_request_lock = self.shared_get_request.lock().await;
                        match &*shared_get_request_lock {
                            SGR::Command {
                                ref device_uuid,
                                ref action,
                            } => {
                                if last_action != (device_uuid.clone(), action.clone()) {
                                    last_action = (device_uuid.clone(), action.clone());
                                    let located_device = located_devices.get(&device_uuid);
                                    /*let _target = match action.get_value() {
                                        Some(v) => v.to_string(),
                                        None => "".to_string(),
                                    };*/
                                    match located_device {
                                        Some(d) => {
                                            let _ = update_device(&d.ip, &device_uuid, &action);
                                            *shared_get_request_lock = SharedGetRequest::NoUpdate;
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
                        let mut shared_ble_command_lock = self.shared_ble_command.lock().await;
                        match &*shared_ble_command_lock {
                            SBC::Command {
                                ref device_uuid,
                                ref action,
                            } => {
                                let mut already_processed = false;
                                for device_group in DEVICE_GROUPS.iter() {
                                    if device_uuid
                                        != &Uuid::from_u128(device_group.uuid_number.clone())
                                    {
                                        continue;
                                    }
                                    for (device_uuid, located_device) in located_devices.iter_mut()
                                    {
                                        if located_device.device.device_group
                                            == Some(device_group.device_group)
                                        {
                                            // TODO: seems like updates could be made in parallel
                                            update_device(
                                                &located_device.ip,
                                                &device_uuid,
                                                &action,
                                            )
                                            .await;
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
                                *shared_ble_command_lock = SBC::NoUpdate;
                            }
                            SBC::Reboot { ref node_count } => {
                                let mut reboot_args = vec!["run".to_string()];
                                if *node_count > 0 {
                                    reboot_args.push("-c".to_string());
                                    reboot_args.push(node_count.to_string());
                                }
                                reboot_args.push("-w".to_string());
                                reboot_args.push("11".to_string());
                                rebooter(reboot_args, self.listen_port.to_string());
                            }
                            SBC::TargetInquiry { ref device_uuid } => {
                                let located_device = located_devices.get(&device_uuid).unwrap();
                                let device = get_device_status_helper(
                                    located_device.ip.clone(),
                                    device_uuid.clone(),
                                )
                                .await;
                                println!("response: {:?}, {:?}", &device_uuid, &device);
                                *shared_ble_command_lock = SBC::TargetResponse {
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
            CLICommand::Shutdown => {
                println!("Shutting down the program!!!");
                let mut stream =
                    TcpStream::connect(format!("127.0.0.1:{}", self.listen_port.to_string()))
                        .unwrap();
                stream
                    .write_all(CLICommand::Shutdown.to_str().as_bytes())
                    .unwrap();
                process::exit(0);
            }
            CLICommand::Status => {
                println!("Getting status...");
                match TcpStream::connect(format!("127.0.0.1:{}", self.listen_port.to_string())) {
                    Ok(mut stream) => {
                        println!("    Running");
                        stream
                            .write_all(CLICommand::Status.to_str().as_bytes())
                            .unwrap();
                    }
                    Err(_) => {
                        println!("    Not running");
                    }
                }
            }
            CLICommand::Reboot => {
                println!("Rebooting...");
                let reboot_args = self.get_reboot_args(&sub_matches);
                rebooter(reboot_args, self.listen_port.clone().to_string());
            }
        }
    }

    /// Spawn a thread to handle the TCP server that's userd for sending/receiving commands between
    /// console windows
    fn start_console_command_sharing(&self, shutdown_flag: Arc<AtomicBool>) {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.listen_port.to_string()))
            .expect("Failed to bind to console_command_sharing address");
        tokio::spawn(async move {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let shutdown_flag_clone = Arc::clone(&shutdown_flag);
                        cli_handler(stream, shutdown_flag_clone).await;
                    }
                    Err(e) => eprintln!("Connection failed: {}", e),
                }
            }
        });
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

    fn run_ble_server(&self, advertising_uuid: Uuid, services: Vec<Service>) {
        let name = self.ble_name.clone();
        tokio::spawn(
            async move { ble_server::run_ble_server(advertising_uuid, services, name).await },
        );
    }

    fn get_reboot_args(&self, sub_matches: &ArgMatches) -> Vec<String> {
        let mut args = Vec::new();

        args.push("run".to_string());

        match sub_matches.get_one::<String>("node-count") {
            Some(nc) => {
                if nc.parse::<u32>().is_err() {
                    println!("You specified that you want to specifiy the number of nodes but you didn't include a positive integer value");
                    process::exit(1);
                }
                args.push("-c".to_string());
                args.push(nc.clone());
            }
            None => {}
        }

        args.push("-w".to_string());
        args.push(self.reboot_wait.to_string());

        args
    }
}

fn rebooter(reboot_args: Vec<String>, listen_port: String) {
    let exicutable_path = env::current_exe().unwrap().to_str().unwrap().to_string();
    println!("    Reboot args: {}, {:?}", &exicutable_path, reboot_args);
    match Command::new(exicutable_path)
        .args(&reboot_args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(_) => println!("        Reboot command issued successfully."),
        Err(e) => eprintln!("        Failed to issue reboot command: {}", e),
    }
    match TcpStream::connect(format!("127.0.0.1:{}", listen_port)) {
        Ok(mut stream) => {
            stream
                .write_all(CLICommand::Reboot.to_str().as_bytes())
                .unwrap();
        }
        Err(_) => {
            println!("    Oops, something went wrong while rebooting...");
        }
    }
    process::exit(0);
}

fn get_ble_services(
    devices: Vec<Device>,
    shared_ble_command: Arc<Mutex<SharedBLECommand>>,
) -> Vec<Service> {
    let mut ble_services = Vec::new();
    let mut set_as_primary = true;
    for device in devices.iter() {
        ble_services.push(ble_server::generic_read_write_service(
            device.uuid,
            Action::Set(0).to_uuid(),
            shared_ble_command.clone(),
            true,
        ));
    }

    ble_services.push(ble_server::voice_service(
        VOICE_UUID,
        shared_ble_command.clone(),
        devices.clone(),
    ));

    ble_services.push(ble_server::hub_reboot_service(
        shared_ble_command.clone(),
        true,
    ));

    ble_services
}
/// Get the list of connected devices if applicable
/// TODO: maybe it should keep track of the max Hashmap of devices so if the limit of 10 tries is
/// hit, that should be what's returned
async fn get_located_devices(node_count: Option<usize>) -> HashMap<Uuid, LocatedDevice> {
    let mut located_devices = HashMap::new();
    println!("Getting Devices!!");
    match node_count {
        Some(nc) => {
            let mut i = 0;
            while located_devices.len() < nc && i < 10 {
                println!("    trying...");
                located_devices = devices::get_devices().await;
                i += 1;
                std::thread::sleep(Duration::from_secs(10));
            }
        }
        None => {
            std::thread::sleep(Duration::from_secs(10));
            located_devices = devices::get_devices().await;
        }
    }

    println!("Devices:");
    for device in located_devices.values() {
        println!("    {}, {}", &device.device.uuid, &device.device.name);
    }

    located_devices
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

async fn cli_handler(mut stream: TcpStream, shutdown_flag: Arc<AtomicBool>) {
    let mut buffer = [0; 1024];
    match stream.read(&mut buffer) {
        Ok(size) => {
            let received = String::from_utf8_lossy(&buffer[..size]);
            let received = received.trim();
            let received = CLICommand::from_str(received);
            match received {
                Ok(command) => match command {
                    CLICommand::Shutdown => {
                        shutdown_flag.store(true, Ordering::SeqCst);
                    }
                    CLICommand::Status => {}
                    CLICommand::Reboot => {
                        shutdown_flag.store(true, Ordering::SeqCst);
                    }
                    CLICommand::Run => {
                        // Shouldn't ever get here
                    }
                },
                Err(e) => {
                    dbg!(e);
                    // TODO: Do I honestly need to worry about this? can this branch be replaced
                    // with a expect? maybe if the previous thing checked
                }
            }
        }
        Err(e) => eprintln!("Failed to receive data: {}", e),
    }
}

fn setup_lock_file() {
    // Get the current working directory
    let exe_path = match env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("Failed to determine current directory: {}", e);
            process::exit(1);
        }
    };

    // Construct the path to the lock file
    let exe_dir = exe_path
        .parent()
        .expect("Couldn't get the parent directory");
    let lock_path = exe_dir.join(LOCK_FILE_NAME);
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

fn check_for_lock_file() -> bool {
    // Get the current working directory
    let current_dir = match env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Failed to determine current directory: {}", e);
            process::exit(1);
        }
    };

    // Construct the path to the lock file
    let lock_path = current_dir.join(LOCK_FILE_NAME);

    lock_path.as_path().exists()
}

fn wait_to_start(sub_matches: &ArgMatches) {
    //let wait_time: &String = match sub_matches.get_one::<u32>("wait") {
    let wait_time: &String = match sub_matches.get_one("wait") {
        Some(wt) => wt,
        None => {
            return ();
        }
    };
    let wait_time = match wait_time.parse() {
        Ok(wt) => wt,
        Err(_) => {
            eprintln!("You specified that you want to wait to start but you didn't include a positive integer value");
            process::exit(1);
        }
    };
    std::thread::sleep(Duration::from_secs(wait_time));
}

pub fn get_user_args() -> clap::Command {
    ClapCommand::new("Hub")
        .version("0.1")
        .author("Chad DeRosier, <chad.derosier@tutanota.com>")
        .about("Runs van automation stuff.")
        .subcommand(
            ClapCommand::new("run")
                .about("Runs the application") // ... additional settings or arguments specific to "run" ...
                .arg(
                    Arg::new("node-count")
                        .short('c')
                        .long("node-count")
                        .action(clap::ArgAction::Set)
                        .help("Set the number of nodes to look for."),
                )
                .arg(
                    Arg::new("wait")
                        .short('w')
                        .long("wait")
                        .action(clap::ArgAction::Set)
                        .help("Wait a period of second before starting. Typically used to allow previous processes to finish or dependencies to get set up."),
                ),
        )
        .subcommand(
            ClapCommand::new("reboot")
                .about("Reboots the main node")
                .arg(
                    Arg::new("node-count")
                        .short('c')
                        .long("node-count")
                        .action(clap::ArgAction::Set)
                        .help("Set the number of nodes to look for."),
                )
        )
        .subcommand(ClapCommand::new("shutdown").about("Shutdown's the program and it's it all down"))
        .subcommand(ClapCommand::new("status").about("Shows some basic status info"))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_wait_to_start() {
        use std::time::{Duration, Instant};

        let sleep_time = 1;
        let sleep_duration = sleep_time.to_string();
        let simulated_args = vec!["Hub", "run", "-w", sleep_duration.as_str()];
        let args = get_user_args().get_matches_from(simulated_args);
        let (_, sub_args) = args.subcommand().unwrap();
        let start = Instant::now();

        wait_to_start(&sub_args);

        let duration = start.elapsed().as_millis();
        assert!((sleep_time * 1000 - 2) < duration);
        assert!(duration < (sleep_time * 1000 + 2));
    }

    #[test]
    fn helper_get_reboot_args() {
        let session = Session {
            ..Default::default()
        };
        let simulated_args = vec!["Hub", "run", "-c", "1"];
        let args = get_user_args().get_matches_from(simulated_args);
        let (_, sub_args) = args.subcommand().unwrap();
        assert_eq!(
            session.get_reboot_args(sub_args),
            vec!["run", "-c", "1", "-w", "10"]
        );

        let simulated_args = vec!["Hub", "run", "-c", "2", "-w", "10"];
        let args = get_user_args().get_matches_from(simulated_args);
        let (_, sub_args) = args.subcommand().unwrap();
        assert_eq!(
            session.get_reboot_args(sub_args),
            vec!["run", "-c", "2", "-w", "10"]
        );
    }
}
