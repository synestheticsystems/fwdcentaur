//! The client projection of the versioned settings contract used by the host and presentation runtime.
use fwd_core::model::TaskStatus;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// The contract bundled with this build, parsed once for every revision it validates.
static CONTRACT: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../deploy/wallboard/wallboard-contract.json"))
        .expect("the bundled settings contract is JSON")
});

/// A value the contract accepts, with its defaults filled in, or why the contract refuses it.
fn validate_settings(value: &Value, spec: &Value, path: &str) -> Result<Value, String> {
    if value.is_null() && spec["nullable"] == true {
        return Ok(Value::Null);
    }
    let number = value.as_f64().filter(|n| n.is_finite());
    let kind = spec["type"].as_str().unwrap();
    let valid = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => number.is_some(),
        "integer" => number.is_some_and(|n| n.fract() == 0.0),
        _ => false,
    };
    if !valid {
        return Err(format!("{path}: expected {kind}"));
    }
    if kind == "object" {
        let mut result = serde_json::Map::new();
        for (key, child) in spec["properties"].as_object().unwrap() {
            result.insert(
                key.clone(),
                validate_settings(value.get(key).unwrap_or(&child["default"]), child, &format!("{path}.{key}"))?,
            );
        }
        if let Some(required) = spec["required_when"].as_object() {
            for (flag, keys) in required {
                let missing: Vec<_> = keys
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|key| result[*key].is_null())
                    .collect();
                if result[flag] == true && !missing.is_empty() {
                    return Err(format!("{path}: {flag} requires {}", missing.join(", ")));
                }
            }
        }
        return Ok(Value::Object(result));
    }
    if kind == "array" {
        let values = value.as_array().unwrap();
        if values.len() < spec["min_length"].as_u64().unwrap_or(0) as usize
            || values.iter().any(|item| !spec["enum"].as_array().unwrap().contains(item))
        {
            return Err(format!("{path}: invalid selection"));
        }
        if spec["unique"] == true && values.iter().enumerate().any(|(i, v)| values[..i].contains(v)) {
            return Err(format!("{path}: duplicate selection"));
        }
    } else if let Some(allowed) = spec["enum"].as_array() {
        if !allowed.iter().any(|v| v == value || (number.is_some() && v.as_f64() == number)) {
            return Err(format!("{path}: unknown value"));
        }
    }
    if let Some(n) = number {
        if n < spec["min"].as_f64().unwrap_or(f64::NEG_INFINITY) || n > spec["max"].as_f64().unwrap_or(f64::INFINITY) {
            return Err(format!("{path}: outside supported range"));
        }
        if kind == "integer" {
            return Ok(json!(n as i64));
        }
    }
    Ok(value.clone())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub enabled: bool,
    pub day_theme: String,
    pub night_theme: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// The settings that choose which tasks the display queries. The store binds them when it opens,
/// for the page's lifetime: another selection needs a new query and a quiet new baseline, never a
/// reactive edit that would present the change as task arrivals and departures.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskSelection {
    pub statuses: Vec<TaskStatus>,
}

/// The settings that change only how the selected tasks are presented, while the display runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Presentation {
    pub theme: String,
    pub theme_schedule: Option<Schedule>,
    pub max_rows: usize,
    pub node_width: usize,
    pub text_scale_percent: usize,
    pub fit_to_screen: bool,
    pub replay_enabled: bool,
    pub replay_interval_seconds: usize,
    pub sound_enabled: bool,
    pub sound_volume: usize,
}

/// One validated settings revision.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub selection: TaskSelection,
    pub presentation: Presentation,
}

impl Default for Settings {
    fn default() -> Self {
        Self::parse(&json!({})).expect("the bundled settings contract's defaults are valid")
    }
}

impl Default for Presentation {
    fn default() -> Self {
        Settings::default().presentation
    }
}

impl Settings {
    pub fn parse(value: &Value) -> Result<Self, String> {
        let v = validate_settings(value, &CONTRACT, "settings")?;
        let schedule = &v["theme_schedule"];
        Ok(Self {
            selection: TaskSelection {
                statuses: v["statuses"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().parse().unwrap())
                    .collect(),
            },
            presentation: Presentation {
                theme: v["theme"].as_str().unwrap().into(),
                theme_schedule: schedule.as_object().map(|_| Schedule {
                    enabled: schedule["enabled"].as_bool().unwrap(),
                    day_theme: schedule["day_theme"].as_str().unwrap().into(),
                    night_theme: schedule["night_theme"].as_str().unwrap().into(),
                    latitude: schedule["latitude"].as_f64(),
                    longitude: schedule["longitude"].as_f64(),
                }),
                max_rows: v["max_rows"].as_u64().unwrap() as usize,
                node_width: v["node_width"].as_u64().unwrap() as usize,
                text_scale_percent: v["text_scale_percent"].as_u64().unwrap() as usize,
                fit_to_screen: v["fit_to_screen"].as_bool().unwrap(),
                replay_enabled: v["replay"]["enabled"].as_bool().unwrap(),
                replay_interval_seconds: v["replay"]["interval_seconds"].as_u64().unwrap() as usize,
                sound_enabled: v["sound"]["enabled"].as_bool().unwrap(),
                sound_volume: v["sound"]["volume"].as_u64().unwrap() as usize,
            },
        })
    }
    /// The complete revision, in the contract's terms: what the presentation runtime validates again.
    pub fn json(&self) -> Value {
        let p = &self.presentation;
        json!({"schema_version":1, "theme":p.theme,
            "theme_schedule":p.theme_schedule.as_ref().map(|s| json!({"enabled":s.enabled,"day_theme":s.day_theme,"night_theme":s.night_theme,"latitude":s.latitude,"longitude":s.longitude})),
            "statuses":self.selection.statuses.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "max_rows":p.max_rows,"node_width":p.node_width,"text_scale_percent":p.text_scale_percent,"fit_to_screen":p.fit_to_screen,
            "replay":{"enabled":p.replay_enabled,"interval_seconds":p.replay_interval_seconds},
            "sound":{"enabled":p.sound_enabled,"volume":p.sound_volume}})
    }
}

impl TaskSelection {
    pub fn keeps(&self, status: Option<TaskStatus>) -> bool {
        status.is_some_and(|s| self.statuses.contains(&s))
    }
    pub fn predicate(&self) -> String {
        format!(
            "created > 0 AND ({})",
            self.statuses.iter().map(|s| format!("status = '{s}'")).collect::<Vec<_>>().join(" OR ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_contract_conformance() {
        let cases: Value = serde_json::from_str(include_str!("../../../deploy/wallboard/settings-cases.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let result = Settings::parse(&case["input"]);
            if case["invalid"] == true {
                assert!(result.is_err(), "{}", case["name"]);
            } else {
                let mut expected = case["expected"].clone();
                for key in ["latitude", "longitude"] {
                    if let Some(n) = expected["theme_schedule"][key].as_f64() {
                        expected["theme_schedule"][key] = json!(n);
                    }
                }
                assert_eq!(result.unwrap().json(), expected, "{}", case["name"]);
            }
        }
    }

    #[test]
    fn an_enabled_schedule_names_the_coordinates_it_is_missing() {
        let missing = Settings::parse(&json!({"theme_schedule": {"enabled": true, "latitude": 51.48}}));
        assert_eq!(missing.unwrap_err(), "settings.theme_schedule: enabled requires longitude");
    }
}
