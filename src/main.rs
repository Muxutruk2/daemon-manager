mod helper;
mod routes;

use minijinja::Environment;
use routes::{handle_service, handle_services};

use std::{
    env::var,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use axum::{Router, routing::get};

use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use systemctl::{SystemCtl, Unit};

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    systemctl: SystemCtl,
    template_env: Arc<minijinja::Environment<'static>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub service: Vec<ServiceConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub service_name: String,
    pub friendly_name: String,

    #[serde(default)]
    pub show_logs: bool,
}

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let config_path: String = var("DAEMON_MANAGER_CONFIG_PATH")
        .map_err(|e| {
            warn!("DAEMON_MANAGER_CONFIG_PATH is not set: {e}. Will use default services.toml")
        })
        .unwrap_or("services.toml".into());

    let config_path: PathBuf = PathBuf::from_str(&config_path).unwrap(); // Infallible

    let config_str = std::fs::read_to_string(config_path.clone())
        .map_err(|e| {
            error!(
                "Could not read configuration file '{}': {e}",
                config_path.display()
            );
            std::process::exit(1);
        })
        .unwrap();

    let config: Config = toml::from_str(&config_str)
        .map_err(|e| {
            error!("Configuration error: {e}");
            std::process::exit(1)
        })
        .unwrap();

    let incorrect = config
        .service
        .iter()
        .map(|v| match v.service_name.rsplit_once('.') {
            Some((_, _)) => true,
            None => {
                error!("Invalid service name: {}", v.service_name);
                false
            }
        })
        .any(|b| b == false);

    if incorrect {
        std::process::exit(1);
    }

    // TODO: Make this for Non-Nix systems
    let systemctl = SystemCtl::builder()
        .path("/run/current-system/sw/bin/systemctl".into())
        .additional_args(Vec::new())
        .build();

    let units = config
        .service
        .iter()
        .filter_map(|s| match systemctl.create_unit(&s.service_name) {
            Ok(unit) => Some(unit),
            Err(e) => {
                error!("Failed to create unit for {}: {}", &s.service_name, e);
                None
            }
        })
        .collect::<Vec<Unit>>();

    let invalid_units = units.iter().any(|unit| match unit.state {
        systemctl::State::Loaded => false,
        systemctl::State::Masked => {
            error!("Unit {} is not loaded (masked or not found)", unit.name);
            true
        }
    });

    if invalid_units {
        error!("Erroneous services found. Exiting");
        std::process::exit(1);
    }

    let mut env = Environment::new();

    env.set_loader(minijinja::path_loader("./templates"));

    let env = Arc::new(env);

    let config = Arc::new(config);

    let state = AppState {
        config: config.clone(),
        systemctl: systemctl.clone(),
        template_env: env,
    };

    let addr: String = var("DAEMON_MANAGER_ADDR")
        .map_err(|e| warn!("DAEMON_MANAGER_ADDR is not set: {e}. Will use default 127.0.0.1:3000"))
        .unwrap_or("127.0.0.1:3000".into());

    let addr: SocketAddrV4 = SocketAddrV4::from_str(&addr)
        .map_err(|e| error!("Could not parse IP addr {addr}: {e}. Will use default 127.0.0.1:3000"))
        .unwrap_or(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 3000));

    let app = Router::new()
        .route("/services", get(handle_services))
        .route("/service/{service}", get(handle_service))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    info!("Listening on {addr}");

    axum::serve(listener, app).await.unwrap();
}

#[derive(Deserialize, Serialize)]
pub struct ServiceInfo {
    config: ServiceConfig,
    status: String,
    active: bool,
    enabled: bool,
    running: bool,
    pid: Option<u64>,
    status_code: Option<u8>,
    uptime: String,
}

#[derive(Deserialize, Serialize)]
pub struct ServiceDetail {
    config: ServiceConfig,
    status: String,
    active: bool,
    enabled: bool,
    running: bool,
    pid: Option<u64>,
    status_code: Option<u8>,
    uptime: String,
    r#type: String,
    unit_file: String,
    processes: Vec<u32>,
    configuration: String,
}
