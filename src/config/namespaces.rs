//! ConfigLoader#add_missing_namespacesの値置換とRegistry警告。表示側の名前補完とは別の段階。

use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use serde_yaml_ng::Value;

use super::ConfigWarning;
use super::validation::OrderedKeys;

/// Registryの曖昧名だけを識別し、汎用I/Oの出力経路へ色/改行を波及させない。
#[derive(Debug)]
pub(crate) struct AmbiguousConfigCopError {
    message: String,
    color: bool,
}

impl fmt::Display for AmbiguousConfigCopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AmbiguousConfigCopError {}

impl AmbiguousConfigCopError {
    pub(crate) fn with_color(mut error: anyhow::Error, color: bool) -> anyhow::Error {
        if let Some(ambiguous) = error.downcast_mut::<Self>() {
            ambiguous.color = color;
        }
        error
    }

    pub(crate) fn color(&self) -> bool {
        self.color
    }
}

struct Registry {
    names: HashSet<String>,
    ordered_names: Vec<String>,
    unqualified: HashSet<String>,
    departments: Vec<String>,
    obsolete: HashSet<String>,
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    // 609名の集合と登録順は実Registryと一致する既存記録を再利用する。
    let entries: Vec<(String, bool)> =
        serde_json::from_str(include_str!("../../config/autocorrect_support.json")).unwrap();
    let mut names = HashSet::new();
    let mut ordered_names = Vec::new();
    let mut unqualified = HashSet::new();
    let mut departments = Vec::new();
    for (name, _) in entries {
        let (department, short) = name.rsplit_once('/').unwrap();
        if !departments.iter().any(|known| known == department) {
            departments.push(department.to_owned());
        }
        unqualified.insert(short.to_owned());
        ordered_names.push(name.clone());
        names.insert(name);
    }
    unqualified.insert("RedundantCopDisableDirective".to_owned());
    let obsolete =
        serde_json::from_str::<Vec<String>>(include_str!("../../config/obsolete_cop_names.json"))
            .unwrap()
            .into_iter()
            .collect();
    Registry {
        names,
        ordered_names,
        unqualified,
        departments,
        obsolete,
    }
});

pub(super) fn registered_or_obsolete(name: &str) -> bool {
    REGISTRY.names.contains(name)
        || REGISTRY
            .departments
            .iter()
            .any(|department| department == name)
        || is_obsolete(name)
}

pub(super) fn is_obsolete(name: &str) -> bool {
    REGISTRY.obsolete.contains(name)
}

pub(super) fn registered_department(name: &str) -> bool {
    REGISTRY
        .departments
        .iter()
        .any(|department| department == name)
}

pub(super) fn suggestion_dictionary() -> impl Iterator<Item = &'static str> {
    REGISTRY
        .departments
        .iter()
        .chain(&REGISTRY.ordered_names)
        .map(String::as_str)
}

pub(super) fn qualified_for_pending(
    name: &str,
    runtime: Option<&super::runtime::Snapshot>,
) -> bool {
    // qualified_cop?は、同じshort名を持つ最初の部門候補とのBadge一致を使う。
    let badge = crate::cop_name::parse_badge(name).0;
    let short = badge.rsplit('/').next().unwrap_or("");
    let departments = runtime.map_or(&REGISTRY.departments, |snapshot| &snapshot.departments);
    departments
        .iter()
        .map(|department| format!("{department}/{short}"))
        .find(|candidate| {
            runtime.map_or_else(
                || REGISTRY.names.contains(candidate),
                |snapshot| snapshot.names.contains(candidate),
            )
        })
        .is_some_and(|candidate| candidate == badge)
}

pub(super) enum Origin<'a> {
    File(&'a Path),
    Remote(&'a Path),
}

pub(super) fn normalize(
    value: &mut Value,
    keys: &mut OrderedKeys,
    origin: Origin<'_>,
    warnings: &mut Vec<ConfigWarning>,
    runtime: &super::RuntimeSession,
) -> Result<String> {
    let Some(mapping) = value.as_mapping_mut() else {
        return Ok(String::new());
    };
    let (full, display) = match origin {
        Origin::File(path) | Origin::Remote(path) => {
            // 生成したcanonicalと入力originはloaderで分け、明示prefixをここで消さない。
            let full = path.to_string_lossy().replace('\\', "/");
            let display = crate::formatter::smart_path(Path::new(&full), &std::env::current_dir()?);
            (full, display)
        }
    };
    if runtime.active() {
        let names = keys
            .names()
            .into_iter()
            .filter(|name| {
                !REGISTRY.obsolete.contains(name)
                    && !matches!(
                        name.as_str(),
                        "require" | "plugins" | "inherit_from" | "inherit_gem"
                    )
            })
            .collect::<Vec<_>>();
        let response = runtime.namespace(&names, &full)?;
        warnings.extend(response.warnings.into_iter().map(ConfigWarning::Namespace));
        for (name, qualified) in names.iter().zip(response.qualified) {
            if *name != qualified {
                let entry = mapping.shift_remove(Value::String(name.clone())).unwrap();
                keys.rename(name, &qualified);
                mapping.insert(Value::String(qualified), entry);
            }
        }
        if let Some(error) = response.error {
            return if error.kind() == "ambiguous" {
                Err(AmbiguousConfigCopError {
                    message: error.message().to_owned(),
                    color: false,
                }
                .into())
            } else {
                Err(error.into())
            };
        }
        return Ok(display);
    }
    if keys.names().iter().any(|name| {
        !REGISTRY.obsolete.contains(name)
            && !matches!(
                name.as_str(),
                "require" | "plugins" | "inherit_from" | "inherit_gem"
            )
            && !crate::cop_name::parse_badge(name).1
    }) {
        runtime.note_unqualified_query();
    }
    // native の merge 後の Mapping 順ではなく、継承時に保存した Ruby Hash の順で走査する。
    for name in keys.names() {
        if REGISTRY.obsolete.contains(&name) {
            continue;
        }
        let qualified = qualify(&name, &full, &display, warnings)?;
        if qualified == name {
            continue;
        }
        // Hash#deleteは他keyを動かさない。既存full節への代入もslotを移さず値全体を置換する。
        let entry = mapping.shift_remove(Value::String(name.clone())).unwrap();
        keys.rename(&name, &qualified);
        mapping.insert(Value::String(qualified), entry);
    }
    Ok(display)
}

fn qualify(
    name: &str,
    origin: &str,
    display: &str,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<String> {
    let (badge, qualified) = crate::cop_name::parse_badge(name);
    if !qualified && REGISTRY.unqualified.contains(name) {
        let advice = if origin.ends_with(".rb") {
            " Run `rubocop -a --only Migration/DepartmentName` to fix."
        } else {
            ""
        };
        warnings.push(ConfigWarning::Namespace(format!(
            "{display}: Warning: no department given for {name}.{advice}\n"
        )));
    }
    // 登録済みBadgeでも返すのは元のString。fully-qualified snake_caseを勝手に受理しない。
    if REGISTRY.names.contains(&badge) {
        return Ok(name.to_owned());
    }
    let short = badge.rsplit('/').next().unwrap_or("");
    let candidates = REGISTRY
        .departments
        .iter()
        .map(|department| format!("{department}/{short}"))
        .filter(|candidate| REGISTRY.names.contains(candidate))
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [] => Ok(name.to_owned()),
        [real] => {
            if qualified {
                warnings.push(ConfigWarning::Namespace(format!(
                    "{display}: Warning: {badge} has the wrong namespace - replace it with {real}\n"
                )));
            }
            Ok(real.clone())
        }
        _ => Err(AmbiguousConfigCopError {
            message: format!(
                "Ambiguous cop name `{badge}` used in {origin} needs department qualifier. Did you mean {}?",
                candidates.join(" or ")
            ),
            color: false,
        }.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::REGISTRY;
    use serde_json::Value;

    #[test]
    fn ambiguous_error_keeps_message_and_context_through_phase_coloring() {
        let mut warnings = Vec::new();
        let error = super::qualify(
            "Wrong/MultipleComparison",
            "config.yml",
            "config.yml",
            &mut warnings,
        )
        .unwrap_err();
        assert!(warnings.is_empty());
        let message = "Ambiguous cop name `Wrong/MultipleComparison` used in config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?";
        assert_eq!(error.to_string(), message);
        assert!(
            !error
                .downcast_ref::<super::AmbiguousConfigCopError>()
                .unwrap()
                .color()
        );
        let colored =
            super::AmbiguousConfigCopError::with_color(error.context("outer context"), true);
        assert_eq!(colored.to_string(), "outer context");
        assert_eq!(
            colored
                .downcast_ref::<super::AmbiguousConfigCopError>()
                .unwrap()
                .to_string(),
            message
        );
        assert!(
            colored
                .downcast_ref::<super::AmbiguousConfigCopError>()
                .unwrap()
                .color()
        );
        let other =
            super::AmbiguousConfigCopError::with_color(anyhow::anyhow!("ordinary I/O"), true);
        assert_eq!(other.to_string(), "ordinary I/O");
        assert!(
            other
                .downcast_ref::<super::AmbiguousConfigCopError>()
                .is_none()
        );
    }

    #[test]
    fn quiet_config_load_keeps_the_existing_ambiguity_refusal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(&path, "Wrong/MultipleComparison: {}\n").unwrap();
        // obsoletionと違いnamespace解決自体が失敗するため、quiet APIでも従来どおり拒否する。
        let error = super::super::Config::load(Some(&path), directory.path()).unwrap_err();
        let typed = error
            .downcast_ref::<super::AmbiguousConfigCopError>()
            .unwrap();
        assert!(!typed.color());
        assert!(
            typed
                .to_string()
                .starts_with("Ambiguous cop name `Wrong/MultipleComparison` used in ")
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"Wrong/MultipleComparison: {}\n"
        );
    }

    fn reference() -> Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/config_cop_namespaces.json"
        ))
        .unwrap()
    }

    #[test]
    fn registry_order_and_obsolete_names_match_the_real_loader() {
        let fixture = reference();
        let order: Vec<(String, bool)> =
            serde_json::from_str(include_str!("../../config/autocorrect_support.json")).unwrap();
        let names = order.into_iter().map(|(name, _)| name).collect::<Vec<_>>();
        assert_eq!(
            serde_json::to_value(names).unwrap(),
            fixture["api"]["registry"]
        );
        let obsolete: Vec<String> =
            serde_json::from_str(include_str!("../../config/obsolete_cop_names.json")).unwrap();
        assert_eq!(
            serde_json::to_value(&obsolete).unwrap(),
            fixture["api"]["legacy"]
        );
        assert_eq!(REGISTRY.obsolete.len(), 67);
    }

    #[test]
    fn public_config_load_resolves_the_short_enabled_section() {
        let fixture = reference();
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == "short")
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        for (name, text) in case["files"].as_object().unwrap() {
            std::fs::write(directory.path().join(name), text.as_str().unwrap()).unwrap();
        }
        let config = super::super::Config::load(
            Some(&directory.path().join("config.yml")),
            directory.path(),
        )
        .unwrap();
        // CLI表示を介さない同じ値解決が必要。原本節を残したまま表示名だけ変えても通らない。
        assert!(config.rule_enabled("Layout/SpaceAroundOperators"));
        assert_eq!(
            config.cop_value::<bool>("Layout/SpaceAroundOperators", "Enabled"),
            Some(true)
        );
        assert!(
            config
                .user
                .as_mapping()
                .unwrap()
                .contains_key("Layout/SpaceAroundOperators")
        );
        assert!(
            !config
                .user
                .as_mapping()
                .unwrap()
                .contains_key("SpaceAroundOperators")
        );
    }
}
