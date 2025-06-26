use std::process::Command;
use std::{
    collections::HashMap,
    env::var,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use sysinfo::System;

use std::time::{Duration, SystemTime};

use axum::{
    Router,
    extract::Path,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};

use log::{debug, error, info, warn};
use minijinja::{Environment, context};
use serde::{Deserialize, Serialize};
use systemctl::{AutoStartStatus, SystemCtl, Unit};

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    systemctl: SystemCtl,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub service: HashMap<String, ServiceConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub service_name: String,
    pub friendly_name: String,
    pub description: Option<String>,

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

    let config_path: PathBuf = PathBuf::from_str(&config_path)
        .map_err(|e| error!("Could not parse config path {config_path}: {e}"))
        .expect("Cannot proceed without config");

    let config_str = std::fs::read_to_string(config_path.clone())
        .map_err(|e| {
            error!(
                "Could not read configuration file '{}': {e}",
                config_path.display()
            )
        })
        .expect("Cannot proceed without config");

    let config: Config = toml::from_str(&config_str)
        .map_err(|e| error!("Configuration error: {e}"))
        .expect("Cannot procced without config");

    let incorrect = config
        .service
        .values()
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
        .values()
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

    let config = Arc::new(config);

    let state = AppState {
        config: config.clone(),
        systemctl: systemctl.clone(),
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

async fn handle_services(State(state): State<AppState>) -> Response {
    let mut env = Environment::new();

    env.add_template(
        "cards",
        r##"
        <div class="services">
        {% for service in services %}
            <div
              class="service-card bg2"
              hx-get="/api/service/{{ service.config.service_name }}"
              hx-target="#detailed-view"
            >
                <h2 class="service-card-name">{{ service.config.friendly_name }}</h2>
                {% if service.active %}
                <p class="service-card-status fg-green">{{ service.status }} (active)</p>
                {% else %}
                <p class="service-card-status fg-yellow">{{ service.status }} (inactive)</p>
                {% endif %}

                {% if service.enabled %}
                <p class="service-card-enabled fg-green">Enabled</p>
                {% else %}
                <p class="service-card-enabled fg-red">Disabled</p>
                {% endif %}

                {% if service.running %}
                <p class="service-card-enabled fg-green">Running ({{ service.pid }})</p>
                <p class="service-card-uptime"> Uptime: {{service.uptime}}</p>
                {% else %}
                <p class="service-card-enabled fg-red">Stopped ({{ service.pid }})</p>
                {% if service.status_code == 0 %}
                <p class="service-card-status-code fg-green"> Status Code {{service.status_code}} </p>
                {% else %}
                <p class="service-card-status-code fg-red"> Status Code {{service.status_code}} </p>
                {% endif %}
                {% endif %}
            </div>
        {% endfor %}
        </div>
    "##,
    )
    .unwrap();

    let units = state
        .config
        .service
        .values()
        .filter_map(|s| match state.systemctl.create_unit(&s.service_name) {
            Ok(unit) => Some(unit),
            Err(e) => {
                error!("Failed to create unit for {}: {}", &s.service_name, e);
                None
            }
        })
        .collect::<Vec<Unit>>();

    let services_info: Vec<ServiceInfo> = units
        .iter()
        .filter_map(|unit| {
            let main_pid = systemd_show("MainPID", &unit.name)
                .map(|c| c.parse::<u64>().unwrap_or(0))
                .unwrap_or(0);

            let status_code =
                systemd_show("StatusErrno", &unit.name).map(|c| c.parse::<u8>().unwrap());

            let uptime = systemd_show("ExecMainStartTimestampMonotonic", &unit.name)
                .map(|a| {
                    a.parse::<u64>()
                        .ok()
                        .expect("Unable to parse ExecMainStartTimestampMonotonic")
                })
                .expect("Unable to get uptime");

            let boot_time = get_boot_time();

            let pretty_uptime = monotonic_uptime(uptime, boot_time);

            match state
                .config
                .service
                .iter()
                .find(|a| a.1.service_name.rsplit_once(".").unwrap().0 == unit.name)
            {
                Some(config) => Some(ServiceInfo {
                    config: config.1.clone(),
                    status: format!("{:?}", unit.state),
                    active: unit.active,
                    enabled: matches!(
                        unit.auto_start,
                        AutoStartStatus::Enabled | AutoStartStatus::EnabledRuntime
                    ),
                    running: main_pid != 0,
                    pid: Some(main_pid),
                    status_code,
                    uptime: pretty_uptime,
                }),
                None => {
                    error!("No service config found for unit '{}'", unit.name);
                    None
                }
            }
        })
        .collect();

    let cards_template = env
        .get_template("cards")
        .map_err(|e| error!("Could not load template 'cards': {e}"));

    if cards_template.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    };

    let response = cards_template
        .unwrap()
        .render(context! {services => services_info})
        .map_err(|e| error!("Could not render template 'cards': {e}"));

    if response.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    }

    Html(response.unwrap()).into_response()
}

fn systemd_show(variable: &str, unit: &str) -> Option<String> {
    Command::new("systemctl")
        .arg("show")
        .arg(unit)
        .arg("--property")
        .arg(variable)
        .arg("--value")
        .output()
        .map_err(|e| error!("Could not run systemctl: {e}"))
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .map(|s| s.trim().to_string())
                    .ok()
            } else {
                error!("systemctl exited with status: {:?}", output.status);
                None
            }
        })
}

fn systemd_status_html(unit: &str) -> Option<String> {
    let output = Command::new("systemctl")
        .arg("status")
        .arg(unit)
        .arg("--no-pager")
        .arg("--lines")
        .arg("0")
        .arg("--full")
        .arg("--legend=no")
        .env("SYSTEMD_COLORS", "1")
        .output()
        .ok()?;

    debug!("Output: {output:?}");

    let raw = String::from_utf8_lossy(&output.stdout);

    debug!("Raw: {raw:?}");
    let html = ansi_to_html::convert(&raw).ok()?;
    debug!("Html: {html:?}");

    Some(html)
}
fn journalctl_html(unit: &str) -> Option<String> {
    let output = Command::new("journalctl")
        .arg("-u")
        .arg(unit)
        .arg("--no-pager")
        .arg("--lines")
        .arg("100")
        .output()
        .ok()?;

    let raw = String::from_utf8_lossy(&output.stdout);
    let html = ansi_to_html::convert(&raw).ok()?;

    debug!("Output: {output:?}");
    debug!("Raw: {raw:?}");
    debug!("Html: {html:?}");

    Some(html)
}

fn systemd_status(unit: &str) -> Option<String> {
    Command::new("systemctl")
        .arg("status")
        .arg(unit)
        .arg("--lines")
        .arg("0")
        .output()
        .map_err(|e| error!("Could not run systemctl: {e}"))
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .map(|s| s.trim().to_string())
                    .ok()
            } else {
                error!("systemctl exited with status: {:?}", output.status);
                None
            }
        })
}
fn monotonic_uptime(monotonic_us: u64, boot_time: SystemTime) -> String {
    let event_time = boot_time + Duration::from_micros(monotonic_us);
    let now = SystemTime::now();
    let diff = now.duration_since(event_time).unwrap_or(Duration::ZERO);
    format_duration(diff.as_secs())
}

fn format_duration(secs: u64) -> String {
    let (days, hours, minutes, seconds) = (
        secs / 86400,
        (secs % 86400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    );

    let mut parts = vec![];
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 || !parts.is_empty() {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 || !parts.is_empty() {
        parts.push(format!("{minutes}m"));
    }
    parts.push(format!("{seconds}s"));

    parts.join(" ")
}

fn get_boot_time() -> std::time::SystemTime {
    let mut sys = System::new();
    sys.refresh_all();

    let boot_time_secs = sysinfo::System::boot_time();

    std::time::UNIX_EPOCH + std::time::Duration::from_secs(boot_time_secs)
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
}

async fn handle_service(Path(service): Path<String>, State(_): State<AppState>) -> Response {
    let mut env = Environment::new();

    env.add_template(
        "commands",
        r##"
        <pre class="command-output">{{ status }}<pre>
        <pre class="command-output">{{ journal }}<pre>
    "##,
    )
    .unwrap();

    let status = systemd_status_html(&service);

    let journal = journalctl_html(&service);

    println!("JOURNAL: {journal:?}");

    let template = env
        .get_template("commands")
        .map_err(|e| error!("Could not load template 'commands': {e}"));

    if template.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    };

    let response = template
        .unwrap()
        .render(context! {status, journal })
        .map_err(|e| error!("Could not render template 'cards': {e}"));

    if response.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    }

    match response {
        Ok(r) => Html(r).into_response(),
        Err(_) => (StatusCode::BAD_REQUEST).into_response(),
    }
}
