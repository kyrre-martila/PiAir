//! A primary choice is one explicitly advertised hardware PCM per CARD/DEV.
//! Aliases remain available verbatim in the advanced list: grouping is a display
//! convenience, not a claim that default, dsnoop and hardware PCMs are equivalent.
use std::collections::BTreeMap;

type Device = (String, String);

fn card_device(id: &str) -> Option<(&str, &str, u32)> {
    let (kind, params) = id.split_once(':')?;
    if !matches!(
        kind,
        "hw" | "plughw" | "front" | "dsnoop" | "default" | "sysdefault"
    ) {
        return None;
    }
    let mut card = None;
    let mut device = None;
    for param in params.split(',') {
        let (key, value) = param.split_once('=')?;
        match key {
            "CARD" if card.is_none() && !value.is_empty() => card = Some(value),
            "DEV" if device.is_none() => device = Some(value.parse::<u32>().ok()?),
            _ => return None, // Unknown routing/subdevice parameters stay explicit.
        }
    }
    let device = match (kind, device) {
        ("default" | "sysdefault", None) => 0,
        (_, Some(device)) => device,
        _ => return None,
    };
    Some((kind, card?, device))
}

pub(super) fn normal_inputs(devices: Vec<Device>) -> Vec<Device> {
    let mut hardware = BTreeMap::new();
    for (id, name) in &devices {
        if let Some((kind @ ("hw" | "plughw"), card, dev)) = card_device(id) {
            let entry = hardware.entry((card, dev)).or_insert((id, name));
            // Select conversion only when that exact PCM was advertised. The
            // selected identifier is still validated; failure never tries hw.
            if kind == "plughw" {
                *entry = (id, name);
            }
        }
    }
    let mut result = Vec::new();
    for (id, name) in &devices {
        if let Some((_, card, dev)) = card_device(id) {
            if let Some((chosen, description)) = hardware.get(&(card, dev)) {
                if id != *chosen {
                    continue;
                }
                let friendly = description.split(',').next().unwrap_or(description).trim();
                let name = if friendly.is_empty() { card } else { friendly };
                let count = hardware.keys().filter(|(c, _)| *c == card).count();
                result.push((
                    id.clone(),
                    if count > 1 {
                        format!("{name} (input {})", u64::from(dev) + 1)
                    } else {
                        name.to_owned()
                    },
                ));
                continue;
            }
        }
        // Virtual, Bluetooth, nonstandard and unrecognized routes retain their
        // own choices. Similar names are never used to merge devices.
        result.push((id.clone(), name.clone()));
    }
    result
}
