# Daemon Manager

Website to visualize systemd services.

# Overview

The service uses caddy to serve static content and Rust for the API. The service is configured using a toml file, which sets the systemd units that will be displayed and managed through the website.

The toml file has this layout:

```toml
[service.nm]
service_name = "NetworkManager.service"
friendly_name = "Network manager"

[service.power-profiles]
service_name = "power-profiles-daemon.service"
friendly_name = "Power profiles"
show-logs = true

[service.display-manager]
service_name = "display-manager.service"
friendly_name = "Display Manager (sddm)"
show-logs = false
```

The API uses the systemctl crate and also runs `systemctl` for missing behaviour. In the future this might change to zbus.

The front-end is HTMX, that is why the API returns HTML.

These are the current API endpoints:

 - **/api/services**: Returns all of the services in a card format
 - **/api/service/{full unit name}**: Returns the systemctl status and journalctl command output of the specified unit

