use anyhow::{Context, Result, anyhow};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use systemctl::{AutoStartStatus, Unit};

use log::{debug, error};
use sysinfo::System;

use crate::{ServiceConfig, ServiceInfo};

pub fn systemd_show_parse<T>(variable: &str, unit: &str) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    Command::new("systemctl")
        .arg("show")
        .arg(unit)
        .arg("--property")
        .arg(variable)
        .arg("--value")
        .output()
        .context("Unable to get STDOUT")
        .and_then(|output| {
            if output.status.success() {
                Ok(String::from_utf8(output.stdout)?
                    .trim_end()
                    .to_owned()
                    .parse::<T>()
                    .context("Unable to parse value")?)
            } else {
                Err(anyhow!(
                    "systemctl failed (status: {:?}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ))
            }
        })
}

pub fn systemd_status_html(unit: &str) -> Result<String> {
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
        .context("Unable to get STDOUT")?;

    let raw =
        String::from_utf8(output.stdout).context("Command output contains Non-UTF8 charachters")?;

    ansi_to_html::convert(&raw).context("Unable to convert command output to HTML")
}

pub fn journalctl_html(unit: &str) -> Result<String> {
    let output = Command::new("journalctl")
        .arg("-u")
        .arg(unit)
        .arg("--no-pager")
        .arg("--lines")
        .arg("100")
        .output()
        .context("Unable to get STDOUT")?;

    let raw =
        String::from_utf8(output.stdout).context("Command output contains Non-UTF8 charachters")?;

    ansi_to_html::convert(&raw).context("Unable to convert command output to HTML")
}

pub fn monotonic_uptime(monotonic_us: u64, boot_time: SystemTime) -> String {
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

pub fn get_boot_time() -> std::time::SystemTime {
    let mut sys = System::new();
    sys.refresh_all();

    let boot_time_secs = sysinfo::System::boot_time();

    std::time::UNIX_EPOCH + std::time::Duration::from_secs(boot_time_secs)
}

pub fn get_unit_info(unit: &Unit, config: Vec<&ServiceConfig>) -> Result<ServiceInfo> {
    let main_pid = systemd_show_parse::<u64>("MainPID", &unit.name).ok();

    let status_code = systemd_show_parse::<u8>("StatusErrno", &unit.name)
        .map_err(|e| error!("StatusCode: {e}"))
        .ok();

    let uptime: u64 = systemd_show_parse::<u64>("ExecMainStartTimestampMonotonic", &unit.name)?;

    let boot_time = get_boot_time();

    let pretty_uptime = monotonic_uptime(uptime, boot_time);

    debug!("Unit Name: {}", unit.name);

    let unit_config = config
        .iter()
        .find(|a| {
            a.service_name
                .rsplit_once(".")
                .map(|n| n.0 == unit.name)
                .unwrap()
        })
        .with_context(|| format!("Unable to get configuration of the service {}", unit.name))?;

    Ok(ServiceInfo {
        config: (*unit_config).clone(),
        status: format!("{:?}", unit.state),
        active: unit.active,
        enabled: matches!(
            unit.auto_start,
            AutoStartStatus::Enabled | AutoStartStatus::EnabledRuntime
        ),
        running: main_pid != Some(0),
        pid: main_pid,
        status_code,
        uptime: pretty_uptime,
    })
}
