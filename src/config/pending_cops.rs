//! Config#pending_copsの通知用schema。rawや実行時のEnabledを変更しない。

use std::borrow::Cow;
use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_yaml_ng::Value;

pub(super) fn pending(
    config: &super::Config,
    mode: Option<bool>,
) -> anyhow::Result<Vec<(String, Option<&Value>)>> {
    let Some(mapping) = config.raw.as_mapping() else {
        return Ok(Vec::new());
    };
    static DEFAULT: LazyLock<Value> = LazyLock::new(|| {
        serde_yaml_ng::from_str(super::DEFAULT_CONFIG)
            .expect("embedded configuration must be valid")
    });
    // mergeのremove/insertで動いたraw順を使わず、本家Hash#mergeが保つ既定slotへ戻す。
    // 明示Ruby拡張のdefault変更も同じ順で適用するが、Config自体は書き換えない。
    let mut schema = Cow::Borrowed(&*DEFAULT);
    if config.runtime.active() {
        config.runtime_snapshot.apply_defaults(schema.to_mut())?;
    }
    let schema = schema
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("pending cop schema is not a mapping"))?;
    let runtime = config.runtime.active().then_some(&*config.runtime_snapshot);
    Ok(schema
        .keys()
        .chain(mapping.keys().filter(|name| !schema.contains_key(*name)))
        .filter_map(|name| {
            let metadata = mapping.get(name)?;
            let name = name.as_str()?;
            if config.cop_schema_enabled_value(name, mode).as_str() != Some("pending")
                || !super::namespaces::qualified_for_pending(name, runtime)
            {
                return None;
            }
            let department = name
                .rsplit_once('/')
                .map_or("", |(department, _)| department);
            if config.raw[department]["Enabled"].as_bool() == Some(false) {
                return None;
            }
            let setting = config.raw[department]
                .get("NewCops")
                .filter(|value| truthy(value))
                .or_else(|| config.raw["AllCops"].get("NewCops"));
            let version = metadata.get("VersionAdded");
            (!covered(setting, version)).then_some((name.to_owned(), version))
        })
        .collect())
}

pub(crate) fn display_version(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => "N/A".to_owned(),
        Some(value) => ruby_scalar(value).unwrap_or_else(|| ruby_inspect(value)),
    }
}

fn ruby_inspect(value: &Value) -> String {
    match value {
        Value::Null => "nil".to_owned(),
        Value::String(value) => serde_json::to_string(value).unwrap(),
        Value::Sequence(values) => format!(
            "[{}]",
            values
                .iter()
                .map(ruby_inspect)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Mapping(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}=>{}", ruby_inspect(key), ruby_inspect(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Tagged(value) => ruby_inspect(&value.value),
        value => ruby_scalar(value).unwrap(),
    }
}

fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

fn covered(setting: Option<&Value>, added: Option<&Value>) -> bool {
    let Some(setting) = setting.and_then(ruby_scalar) else {
        return false;
    };
    match setting.as_str() {
        "enable" | "disable" => true,
        "" | "pending" => false,
        _ => match (
            version(&setting),
            added.and_then(ruby_scalar).and_then(|s| version(&s)),
        ) {
            (Some(pinned), Some(added)) => compare(&added, &pinned) != Ordering::Greater,
            _ => false,
        },
    }
}

fn ruby_scalar(value: &Value) -> Option<String> {
    if let Ok(text) = String::deserialize(value) {
        return Some(text);
    }
    match value {
        Value::Null => Some(String::new()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => {
            // MRIは小さいFloatを1.0e-09の形で渡す。RubyGemsは文字列の数字列と
            // prereleaseを比較するため、数値の大小へ置き換えてはいけない。
            if value.is_f64()
                && let Some(float) = value.as_f64()
                && float != 0.0
                && float.abs() < 0.0001
            {
                let text = format!("{float:e}");
                let (mantissa, exponent) = text.split_once('e')?;
                let mantissa = if mantissa.contains('.') {
                    mantissa.to_owned()
                } else {
                    format!("{mantissa}.0")
                };
                let exponent: i32 = exponent.parse().ok()?;
                Some(format!("{mantissa}e{exponent:+03}"))
            } else {
                Some(value.to_string())
            }
        }
        Value::Tagged(value) => ruby_scalar(&value.value),
        // Array/HashのRuby表現は数字から始まらずcomparable_versionはnilになる。
        _ => None,
    }
}

#[derive(Debug, PartialEq)]
enum Segment {
    Number(String),
    Text(String),
}

fn version(text: &str) -> Option<Vec<Segment>> {
    static VALID: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\A[0-9]+(?:\.[0-9a-zA-Z]+)*(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\z").unwrap()
    });
    if !text.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let text = text.trim_end_matches(|c: char| c.is_ascii_whitespace());
    if !VALID.is_match(text) {
        return None;
    }
    let text = text.replace('-', ".pre.");
    let mut parts = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(first) = chars.next() {
        if !first.is_ascii_alphanumeric() {
            continue;
        }
        let numeric = first.is_ascii_digit();
        let mut part = first.to_string();
        while chars
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() && c.is_ascii_digit() == numeric)
        {
            part.push(chars.next().unwrap());
        }
        parts.push(if numeric {
            Segment::Number(part.trim_start_matches('0').to_owned())
        } else {
            Segment::Text(part)
        });
    }
    // Gem::Versionのcanonical_segmentsは末尾と最初の文字列直前の0を除く。
    while matches!(parts.last(), Some(Segment::Number(value)) if value.is_empty()) {
        parts.pop();
    }
    if let Some(mut index) = parts
        .iter()
        .position(|part| matches!(part, Segment::Text(_)))
    {
        while index > 0 && matches!(&parts[index - 1], Segment::Number(value) if value.is_empty()) {
            parts.remove(index - 1);
            index -= 1;
        }
    }
    Some(parts)
}

fn compare(left: &[Segment], right: &[Segment]) -> Ordering {
    let zero = Segment::Number(String::new());
    for index in 0..left.len().max(right.len()) {
        let order = match (
            left.get(index).unwrap_or(&zero),
            right.get(index).unwrap_or(&zero),
        ) {
            (Segment::Number(left), Segment::Number(right)) => {
                left.len().cmp(&right.len()).then_with(|| left.cmp(right))
            }
            (Segment::Text(left), Segment::Text(right)) => left.cmp(right),
            (Segment::Number(_), Segment::Text(_)) => Ordering::Greater,
            (Segment::Text(_), Segment::Number(_)) => Ordering::Less,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use serde_json::Value as Json;
    use serde_yaml_ng::Value;

    use super::{covered, display_version, ruby_scalar};
    use crate::config::Config;

    fn fixture() -> Json {
        serde_json::from_str(include_str!("../../tests/fixtures/pending_cops.json")).unwrap()
    }

    #[test]
    fn raw_default_schema_and_notice_order_match_actual_upstream() {
        let data = fixture();
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load_with_options(None, directory.path(), true).unwrap();
        let before = (
            config.raw.clone(),
            config.user.clone(),
            *config.cache_digest().unwrap(),
        );
        for row in data["primary"]["table"].as_array().unwrap() {
            let metadata = &config.raw[row["name"].as_str().unwrap()];
            assert_eq!(
                serde_json::to_value(metadata.get("Enabled")).unwrap(),
                row["enabled"],
                "{}",
                row["name"]
            );
            assert_eq!(
                serde_json::to_value(metadata.get("VersionAdded")).unwrap(),
                row["version_added"],
                "{}",
                row["name"]
            );
        }
        let pending = config.pending_cops(None).unwrap();
        assert_eq!(pending.len(), 159);
        for ((name, added), row) in pending
            .iter()
            .zip(data["primary"]["pending"].as_array().unwrap())
        {
            assert_eq!(*name, row["name"].as_str().unwrap());
            assert_eq!(serde_json::to_value(added).unwrap(), row["version_added"]);
        }
        assert_eq!(
            before,
            (
                config.raw.clone(),
                config.user.clone(),
                *config.cache_digest().unwrap()
            )
        );
    }

    #[test]
    fn notice_view_models_defaults_without_changing_runtime_values() {
        let data = fixture();
        let mut count = 0;
        for case in data["cases"].as_array().unwrap() {
            let args = case["args"].as_array().unwrap();
            let has = |flag| args.iter().any(|arg| arg == flag);
            // これらはCLIのearly exit/Reporter flagであり、schemaのpredicateとは別条件。
            if [
                "--version",
                "--verbose-version",
                "--show-cops",
                "--show-docs-url",
                "--list-enabled-cops-for",
                "--enable-pending-cops",
                "--disable-pending-cops",
            ]
            .iter()
            .any(|flag| has(*flag))
            {
                continue;
            }
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join("Gemfile"), "").unwrap();
            let path = directory.path().join("config.yml");
            std::fs::write(&path, case["files"]["config.yml"].as_str().unwrap()).unwrap();
            let config = Config::load_with_options(
                Some(&path),
                directory.path(),
                has("--force-default-config"),
            )
            .unwrap();
            let before = (
                config.raw.clone(),
                config.user.clone(),
                config.rule_enabled("Lint/DeprecatedReference"),
                *config.cache_digest().unwrap(),
            );
            let default = if has("--disable-all-cops") {
                Some(false)
            } else if has("--enable-all-cops") {
                Some(true)
            } else {
                None
            };
            let names = config
                .pending_cops(default)
                .unwrap()
                .into_iter()
                .map(|(name, _)| name.to_owned())
                .collect::<Vec<_>>();
            assert_eq!(
                serde_json::to_value(names).unwrap(),
                case["pending_names"],
                "{}",
                case["id"]
            );
            assert_eq!(
                before,
                (
                    config.raw.clone(),
                    config.user.clone(),
                    config.rule_enabled("Lint/DeprecatedReference"),
                    *config.cache_digest().unwrap()
                )
            );
            count += 1;
        }
        assert_eq!(count, 27);
    }

    #[test]
    fn version_coverage_matches_real_gem_version_components() {
        let data = fixture();
        let pinned = Value::String("1.85".to_owned());
        for row in data["version_api"]["rows"].as_array().unwrap() {
            let value = serde_yaml_ng::to_value(&row["value"]).unwrap();
            assert_eq!(
                ruby_scalar(&value).unwrap_or_default(),
                row["ruby_to_s"].as_str().unwrap(),
                "{row}"
            );
            let display = if value.is_null() || value.as_bool() == Some(false) {
                "N/A"
            } else {
                row["ruby_to_s"].as_str().unwrap()
            };
            assert_eq!(display_version(Some(&value)), display, "{row}");
            assert_eq!(
                covered(Some(&pinned), Some(&value)),
                row["covered_by_1_85"].as_bool().unwrap(),
                "{row}"
            );
        }
    }

    #[test]
    fn qualified_predicate_matches_real_registry_and_negative_names() {
        let data = fixture();
        let rows = data["qualified_api"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 614);
        for row in rows {
            assert_eq!(
                crate::config::namespaces::qualified_for_pending(row[0].as_str().unwrap(), None),
                row[1].as_bool().unwrap(),
                "{}",
                row[0]
            );
        }
    }
}
