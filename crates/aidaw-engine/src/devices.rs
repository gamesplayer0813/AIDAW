//! Sound drivers (cpal "hosts": WASAPI, ASIO when built with it, ALSA on
//! Linux) and the devices each one offers, by name.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait};

/// Names of the drivers this build can use, the system's default first
/// (on Windows: "WASAPI", then "ASIO" when built with the `asio` feature).
pub fn drivers() -> Vec<&'static str> {
    let default = cpal::default_host().id();
    let mut ids = cpal::available_hosts();
    ids.sort_by_key(|id| *id != default);
    ids.iter().map(|id| id.name()).collect()
}

/// Whether this build can talk to ASIO drivers.
pub fn has_asio() -> bool {
    drivers().iter().any(|d| d.eq_ignore_ascii_case("asio"))
}

/// The driver called `name` (any case), or the system default for `None`.
pub(crate) fn host(driver: Option<&str>) -> Result<cpal::Host> {
    let Some(name) = driver else {
        return Ok(cpal::default_host());
    };
    let id = cpal::available_hosts()
        .into_iter()
        .find(|id| id.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow!("the {name} driver is not available in this build"))?;
    cpal::host_from_id(id).map_err(|e| anyhow!("can't start the {name} driver: {e}"))
}

pub(crate) fn name_of(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "unknown device".into())
}

/// Output devices of a driver, the system default first.
pub fn output_devices(driver: Option<&str>) -> Vec<String> {
    let Ok(host) = host(driver) else {
        return Vec::new();
    };
    let default = host.default_output_device().map(|d| name_of(&d));
    let names = host
        .output_devices()
        .map(|it| it.map(|d| name_of(&d)).collect())
        .unwrap_or_default();
    default_first(names, default)
}

/// Input devices (microphones, line inputs) of a driver, the default first.
pub fn input_devices(driver: Option<&str>) -> Vec<String> {
    let Ok(host) = host(driver) else {
        return Vec::new();
    };
    let default = host.default_input_device().map(|d| name_of(&d));
    let names = host
        .input_devices()
        .map(|it| it.map(|d| name_of(&d)).collect())
        .unwrap_or_default();
    default_first(names, default)
}

fn default_first(mut names: Vec<String>, default: Option<String>) -> Vec<String> {
    if let Some(d) = default {
        names.retain(|n| *n != d);
        names.insert(0, d);
    }
    names
}

/// Names of all output devices of the default driver, the default first.
pub fn output_device_names() -> Vec<String> {
    output_devices(None)
}

pub(crate) fn find_output(host: &cpal::Host, name: &str) -> Result<cpal::Device> {
    host.output_devices()?
        .find(|d| name_of(d) == name)
        .ok_or_else(|| anyhow!("output device {name:?} not found"))
}

pub(crate) fn find_input(host: &cpal::Host, name: &str) -> Result<cpal::Device> {
    host.input_devices()?
        .find(|d| name_of(d) == name)
        .ok_or_else(|| anyhow!("input device {name:?} not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_device_comes_first() {
        let names = vec!["A".to_string(), "B".into(), "C".into()];
        assert_eq!(
            default_first(names.clone(), Some("B".into())),
            ["B", "A", "C"]
        );
        assert_eq!(default_first(names.clone(), None), ["A", "B", "C"]);
        assert_eq!(default_first(names, Some("D".into())), ["D", "A", "B", "C"]);
    }

    #[test]
    fn unknown_drivers_are_refused() {
        assert!(host(Some("no such driver")).is_err());
        assert!(host(None).is_ok());
        // Whatever this machine has, its default driver is listed first.
        assert_eq!(drivers()[0], cpal::default_host().id().name());
    }
}
