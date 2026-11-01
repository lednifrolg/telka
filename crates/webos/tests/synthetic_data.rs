//! Guards for the synthetic test data in `tests/data`.
//!
//! Each file there is one SSAP frame as the TV sends it, hand-written to follow the TV's schema
//! and holding no captured values. When `WEBOS_CAPTURED_DIR` points to a directory of captured
//! frames with the same file names, every field in a synthetic frame must also appear in the
//! captured one with the same JSON type, so the test data cannot drift from what the TV actually
//! sends, and no identifier from the captured frames may appear in the synthetic data. Without
//! it, including in CI, those checks are skipped, and so is every frame that has no capture yet.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Fields whose captured values identify a real TV or its pairing.
const IDENTIFYING: [&str; 5] = [
    "deviceUUID",
    "client-key",
    "serialNumber",
    "macAddress",
    "socketPath",
];

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// File names of the synthetic frames, sorted.
fn synthetic_files() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(data_dir())
        .expect("tests/data")
        .map(|entry| entry.expect("tests/data entry").file_name())
        .map(|name| name.into_string().expect("UTF-8 file name"))
        .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "json"))
        .collect();
    names.sort();
    names
}

/// Synthetic frames that have a captured counterpart, as (file name, captured path).
fn captured_pairs() -> Vec<(String, PathBuf)> {
    let Some(dir) = std::env::var_os("WEBOS_CAPTURED_DIR").map(PathBuf::from) else {
        eprintln!("skipped: WEBOS_CAPTURED_DIR is not set");
        return Vec::new();
    };
    let (pairs, missing): (Vec<_>, Vec<_>) = synthetic_files()
        .into_iter()
        .map(|name| {
            let captured = dir.join(&name);
            (name, captured)
        })
        .partition(|(_, captured)| captured.is_file());
    for (name, _) in &missing {
        eprintln!("skipped: no captured {name}");
    }
    pairs
}

/// Collects every place where `synthetic` has a field that `captured` lacks or types differ.
/// `null` on either side matches anything, because nullable fields carry no type.
/// Arrays can mix kinds of items, such as inputs with and without a CEC device, so each synthetic
/// item is held against the captured item it fits best. An empty captured array says nothing
/// about its items, so those are not checked.
fn schema_drift(synthetic: &Value, captured: &Value, path: &str, drift: &mut Vec<String>) {
    match (synthetic, captured) {
        (Value::Null, _)
        | (_, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => {}
        (Value::Object(fields), Value::Object(known)) => {
            for (key, value) in fields {
                let child = format!("{path}.{key}");
                match known.get(key) {
                    Some(known_value) => schema_drift(value, known_value, &child, drift),
                    None => drift.push(format!("{child} is not in the captured frame")),
                }
            }
        }
        (Value::Array(items), Value::Array(known)) => {
            for (i, item) in items.iter().enumerate() {
                let child = format!("{path}[{i}]");
                let best = known
                    .iter()
                    .map(|example| {
                        let mut found = Vec::new();
                        schema_drift(item, example, &child, &mut found);
                        found
                    })
                    .min_by_key(Vec::len);
                drift.extend(best.unwrap_or_default());
            }
        }
        _ => drift.push(format!("{path} has a different JSON type than captured")),
    }
}

/// Shorter values, such as an empty socket path, would match by accident.
const MIN_IDENTIFIER_LEN: usize = 8;

/// Gathers the string values of identifying fields anywhere in `value`.
fn identifiers(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields {
                match field {
                    Value::String(text)
                        if IDENTIFYING.contains(&key.as_str())
                            && text.len() >= MIN_IDENTIFIER_LEN =>
                    {
                        found.push(text.clone());
                    }
                    _ => identifiers(field, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| identifiers(item, found)),
        _ => {}
    }
}

#[test]
fn synthetic_frames_are_valid_json() {
    let names = synthetic_files();
    assert!(!names.is_empty(), "tests/data holds no frames");
    for name in names {
        let frame = read_json(&data_dir().join(&name));
        assert!(
            frame.get("type").is_some_and(Value::is_string),
            "{name} has no frame type"
        );
    }
}

#[test]
fn synthetic_frames_match_captured_schema() {
    let mut drift = Vec::new();
    for (name, captured) in captured_pairs() {
        let synthetic = read_json(&data_dir().join(&name));
        schema_drift(&synthetic, &read_json(&captured), &name, &mut drift);
    }
    assert!(
        drift.is_empty(),
        "synthetic data drifted from the API schema:\n{}",
        drift.join("\n")
    );
}

#[test]
fn synthetic_frames_hold_no_captured_identifiers() {
    // Any captured identifier is a leak in any synthetic frame, not only in its counterpart.
    let mut found = Vec::new();
    for (_, captured) in captured_pairs() {
        identifiers(&read_json(&captured), &mut found);
    }
    for name in synthetic_files() {
        let text = fs::read_to_string(data_dir().join(&name)).expect("synthetic frame");
        let leaked = found.iter().filter(|id| text.contains(id.as_str())).count();
        assert!(leaked == 0, "{name} holds {leaked} captured identifiers");
    }
}
