//! ConfigValidator#validate_new_cops_parameterの許容型。pending copの有効判定とは分ける。

use std::fmt;

use serde_yaml_ng::Value;

use super::runtime::Snapshot;

#[derive(Clone, Debug)]
pub(crate) struct InvalidNewCopsError {
    message: String,
    color: bool,
}

impl InvalidNewCopsError {
    pub(crate) fn color(&self) -> bool {
        self.color
    }

    pub(crate) fn with_color(mut error: anyhow::Error, color: bool) -> anyhow::Error {
        if let Some(invalid) = error.downcast_mut::<Self>() {
            invalid.color = color;
        }
        error
    }
}

impl fmt::Display for InvalidNewCopsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for InvalidNewCopsError {}

pub(super) fn validation_error<'a>(
    config: &Value,
    display: &str,
    names: impl IntoIterator<Item = &'a str>,
    registry: &Snapshot,
) -> Option<InvalidNewCopsError> {
    let mapping = config.as_mapping()?;
    if let Some(value) = mapping.get("AllCops").and_then(new_cops)
        && !enum_or_nil(value)
    {
        return Some(invalid(value, display, false));
    }
    // merge後のMappingはslotが動くため、本家Hashの検証順を別に受け取る。
    for name in names {
        if (super::namespaces::registered_department(name)
            || registry
                .departments
                .iter()
                .any(|department| department == name))
            && let Some(value) = mapping.get(name).and_then(new_cops)
            && !enum_or_nil(value)
            && !department_version(value)
        {
            return Some(invalid(value, display, true));
        }
    }
    None
}

fn enum_or_nil(value: &Value) -> bool {
    matches!(value, Value::Null)
        || matches!(value, Value::String(value) if matches!(value.as_str(), "pending" | "disable" | "enable"))
}

fn new_cops(section: &Value) -> Option<&Value> {
    section
        .as_mapping()
        .and_then(|section| section.get("NewCops"))
}

fn department_version(value: &Value) -> bool {
    match value {
        Value::Number(_) => true,
        Value::String(value) => value
            .as_bytes()
            .split(|byte| *byte == b'.')
            .all(|part| !part.is_empty() && part.iter().all(u8::is_ascii_digit)),
        _ => false,
    }
}

fn invalid(value: &Value, display: &str, department: bool) -> InvalidNewCopsError {
    let choices = if department {
        "Valid choices for a department are: pending, disable, enable, or a version string like '1.50'"
    } else {
        "Valid choices are: pending, disable, enable"
    };
    InvalidNewCopsError {
        message: format!(
            "invalid {} for `NewCops` found in {display}\n{choices}",
            ruby_value(value, false)
        ),
        color: false,
    }
}

fn ruby_value(value: &Value, inspecting: bool) -> String {
    match value {
        Value::Null => "nil".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => match value.as_f64() {
            Some(number) if number.is_nan() => "NaN".to_owned(),
            Some(number) if number == f64::INFINITY => "Infinity".to_owned(),
            Some(number) if number == f64::NEG_INFINITY => "-Infinity".to_owned(),
            Some(number)
                if value.is_f64()
                    && number != 0.0
                    && (number.abs() < 0.0001 || number.abs() >= 1e16) =>
            {
                // MRIのFloat#to_sはmantissaに小数点、exponentに符号と最低2桁を保つ。
                let text = format!("{number:e}");
                let (mantissa, exponent) = text.split_once('e').unwrap();
                let mantissa = if mantissa.contains('.') {
                    mantissa.to_owned()
                } else {
                    format!("{mantissa}.0")
                };
                let exponent: i32 = exponent.parse().unwrap();
                format!("{mantissa}e{exponent:+03}")
            }
            _ => value.to_string(),
        },
        Value::String(text) if !inspecting => text.clone(),
        Value::String(text) => quoted_string(text),
        Value::Sequence(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| ruby_value(value, true))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Mapping(mapping) => format!(
            "{{{}}}",
            mapping
                .iter()
                .map(|(key, value)| format!(
                    "{} => {}",
                    ruby_value(key, true),
                    ruby_value(value, true)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Tagged(tagged) => ruby_value(&tagged.value, inspecting),
    }
}

fn quoted_string(text: &str) -> String {
    // String#to_sは原文のまま、Array/Hash内だけString#inspectで引用する。
    let mut output = String::from("\"");
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{7}' => output.push_str("\\a"),
            '\u{8}' => output.push_str("\\b"),
            '\u{b}' => output.push_str("\\v"),
            '\u{c}' => output.push_str("\\f"),
            '\u{1b}' => output.push_str("\\e"),
            '#' if chars
                .peek()
                .is_some_and(|next| matches!(*next, '{' | '@' | '$')) =>
            {
                output.push_str("\\#")
            }
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04X}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_scalar_display_and_order_match_fifty_actual_conditions() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/new_cops_validation.json"
        ))
        .unwrap();
        let mut checked = 0;
        for (index, row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let id = row["id"].as_str().unwrap();
            if !(id.starts_with("AllCops-")
                || id.starts_with("Lint-")
                || id.starts_with("department-")
                || matches!(id, "array-inspect" | "all-before-departments")
                || (index >= 64 && row["requires_ruby"] != true))
            {
                continue;
            }
            let yaml = super::super::yaml_booleans::normalize(
                row["files"]["config.yml"].as_str().unwrap(),
            );
            let value: Value = serde_yaml_ng::from_str(&yaml).unwrap();
            let keys = super::super::validation::OrderedKeys::from_value(&value).names();
            let error = validation_error(
                &value,
                "config.yml",
                keys.iter().map(String::as_str),
                &Snapshot::default(),
            );
            if row["kind"] == "accepted" {
                assert!(error.is_none(), "{id}: {error:?}");
            } else {
                let hex = row["stderr_hex"].as_str().unwrap();
                let bytes: Vec<_> = hex
                    .as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|part| u8::from_str_radix(std::str::from_utf8(part).unwrap(), 16).unwrap())
                    .collect();
                let expected = String::from_utf8(bytes).unwrap().replace("\r\n", "\n");
                assert_eq!(
                    error.unwrap().to_string(),
                    expected
                        .strip_prefix("Error: ")
                        .unwrap()
                        .strip_suffix('\n')
                        .unwrap(),
                    "{id}"
                );
            }
            checked += 1;
        }
        assert_eq!(checked, 50);
    }

    #[test]
    fn color_annotation_preserves_context_and_other_error_types() {
        let error: anyhow::Error = InvalidNewCopsError {
            message: "kept".to_owned(),
            color: false,
        }
        .into();
        assert!(!error.downcast_ref::<InvalidNewCopsError>().unwrap().color());
        let error = InvalidNewCopsError::with_color(error.context("outer"), true);
        assert_eq!(format!("{error:#}"), "outer: kept");
        assert!(error.downcast_ref::<InvalidNewCopsError>().unwrap().color());
        let other = InvalidNewCopsError::with_color(anyhow::anyhow!("ordinary"), true);
        assert_eq!(other.to_string(), "ordinary");
        assert!(!other.is::<InvalidNewCopsError>());
    }
}
