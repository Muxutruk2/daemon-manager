use crate::helper::*;

use axum::{
    extract::Path,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use anyhow::Context;
use log::error;
use minijinja::{Environment, context};
use systemctl::{AutoStartStatus, Unit};

use crate::{AppState, ServiceInfo};

pub async fn handle_services(State(state): State<AppState>) -> Response {
    let env = state.template_env;

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
        .into_iter()
        .filter_map(|unit| {
            get_unit_info(&unit, state.config.service.values().collect())
                .map_err(|e| error!("Error geting unit info: {e}"))
                .ok()
        })
        .collect();

    let cards_template = env
        .get_template("cards")
        .context("Could not load template 'cards'");

    match cards_template {
        Ok(_) => {}
        Err(e) => {
            error!("Could not get template 'cards': {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
        }
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

pub async fn handle_service(
    Path(service): Path<String>,
    State(state): State<AppState>,
) -> Response {
    let env = state.template_env;

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
