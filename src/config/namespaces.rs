//! ConfigLoader#add_missing_namespacesの値置換とRegistry警告。表示側の名前補完とは別の段階。

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, bail};
use serde_yaml_ng::Value;

use super::ConfigWarning;
use super::validation::OrderedKeys;

struct Registry {
    names: HashSet<String>,
    unqualified: HashSet<String>,
    departments: Vec<String>,
    obsolete: HashSet<String>,
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    // 609名の集合と登録順は実Registryと一致する既存記録を再利用する。
    let entries: Vec<(String, bool)> =
        serde_json::from_str(include_str!("../../config/autocorrect_support.json")).unwrap();
    let mut names = HashSet::new();
    let mut unqualified = HashSet::new();
    let mut departments = Vec::new();
    for (name, _) in entries {
        let (department, short) = name.rsplit_once('/').unwrap();
        if !departments.iter().any(|known| known == department) {
            departments.push(department.to_owned());
        }
        unqualified.insert(short.to_owned());
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
        unqualified,
        departments,
        obsolete,
    }
});

pub(super) enum Origin<'a> {
    File(&'a Path),
    Remote(&'a str),
}

pub(super) fn normalize(
    value: &mut Value,
    keys: &mut OrderedKeys,
    origin: Origin<'_>,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<()> {
    let Some(mapping) = value.as_mapping_mut() else {
        return Ok(());
    };
    let (full, display) = match origin {
        Origin::File(path) => {
            // canonicalizeのWindows API prefixはRubyの表示に含まれない。
            let full = path.to_string_lossy().replace('\\', "/");
            let full = if let Some(unc) = full.strip_prefix("//?/UNC/") {
                format!("//{unc}")
            } else {
                full.strip_prefix("//?/").unwrap_or(&full).to_owned()
            };
            let display = crate::formatter::smart_path(Path::new(&full), &std::env::current_dir()?);
            (full, display)
        }
        Origin::Remote(url) => (url.to_owned(), url.to_owned()),
    };
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
    Ok(())
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
        _ => bail!(
            "Ambiguous cop name `{badge}` used in {origin} needs department qualifier. Did you mean {}?",
            candidates.join(" or ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::REGISTRY;
    use serde_json::Value;

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
