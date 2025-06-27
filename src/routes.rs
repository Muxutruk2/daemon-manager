use crate::helper::*;

use axum::{
    extract::Path,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use log::error;
use minijinja::{Environment, context};
use systemctl::{AutoStartStatus, Unit};

use crate::{AppState, ServiceInfo};

pub async fn handle_services(State(state): State<AppState>) -> Response {
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

            let status_code = systemd_show_parse::<u8>("StatusErrno", &unit.name)
                .map_err(|e| error!("{e}"))
                .ok();

            let uptime: u64 = systemd_show("ExecMainStartTimestampMonotonic", &unit.name)
                .map(|a| {
                    a.parse::<u64>()
                        .map_err(|e| error!("Unable to parse ExecMainStartTimestampMonotonic: {e}"))
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

pub async fn handle_service(Path(service): Path<String>, State(_): State<AppState>) -> Response {
    let mut env = Environment::new();

    env.add_template(
        "commands",
        r##"
        <pre class="command-output">{{ status }}<pre>
        <pre class="command-output">{{ journal }}<pre>
    "##,
    )
    .unwrap();

    let status = systemd_status_html(&service)
        .map_err(|e| error!("{e}"))
        .ok();

    let journal = journalctl_html(&service).map_err(|e| error!("{e}")).ok();

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
