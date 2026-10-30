use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use anyhow::{Result, bail};
use serde_yaml_ng::Value;

use super::runtime::Snapshot;

const COMMON: &[&str] = &[
    "Exclude",
    "Include",
    "Severity",
    "inherit_mode",
    "AutoCorrect",
    "StyleGuide",
    "Details",
    "Enabled",
    "Reference",
    "References",
    "Safe",
    "SafeAutoCorrect",
];
const INTERNAL: &[&str] = &[
    "Description",
    "StyleGuide",
    "VersionAdded",
    "VersionChanged",
    "VersionRemoved",
    "Reference",
    "References",
    "Safe",
    "SafeAutoCorrect",
];

struct Parameters {
    allowed: HashSet<String>,
    supported: String,
}

static SCHEMA: LazyLock<HashMap<String, Parameters>> = LazyLock::new(|| {
    let defaults: Value =
        serde_yaml_ng::from_str(super::DEFAULT_CONFIG).expect("組み込みの既定設定は検証済みである");
    defaults
        .as_mapping()
        .unwrap()
        .iter()
        .filter_map(|(name, settings)| {
            let name = name.as_str()?;
            let keys: Vec<_> = settings
                .as_mapping()?
                .keys()
                .filter_map(Value::as_str)
                .collect();
            let supported = keys
                .iter()
                .copied()
                .filter(|key| !INTERNAL.contains(key))
                .collect::<Vec<_>>()
                .join("\n  - ");
            Some((
                name.to_owned(),
                Parameters {
                    allowed: keys.into_iter().map(ToOwned::to_owned).collect(),
                    supported,
                },
            ))
        })
        .collect()
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Enabled {
    Missing,
    Nil,
    No,
    Yes,
    Other,
}

impl Enabled {
    fn from_value(value: Option<&Value>) -> Self {
        match value {
            None => Self::Missing,
            Some(Value::Null) => Self::Nil,
            Some(Value::Bool(false)) => Self::No,
            Some(Value::Bool(true)) => Self::Yes,
            Some(_) => Self::Other,
        }
    }

    fn truthy(self) -> bool {
        matches!(self, Self::Yes | Self::Other)
    }
}

#[derive(Clone, Debug)]
struct SectionKeys {
    name: String,
    parameters: Option<Vec<Value>>,
    enabled: Enabled,
}

/// Ruby Hash の検証順を値とは別に保存し、部門による並べ替えと名前の置換を再現する。
#[derive(Clone, Debug, Default)]
pub(super) struct OrderedKeys {
    sections: Vec<SectionKeys>,
}

impl OrderedKeys {
    pub(super) fn from_value(value: &Value) -> Self {
        let sections = value
            .as_mapping()
            .into_iter()
            .flat_map(|mapping| mapping.iter())
            .filter_map(|(name, value)| {
                Some(SectionKeys {
                    name: name.as_str()?.to_owned(),
                    parameters: value
                        .as_mapping()
                        .map(|mapping| mapping.keys().cloned().collect()),
                    enabled: Enabled::from_value(
                        value
                            .as_mapping()
                            .and_then(|mapping| mapping.get("Enabled")),
                    ),
                })
            })
            .collect();
        Self { sections }
    }

    pub(super) fn inherit(&mut self, base: &Self) {
        let disabled: HashSet<String> = self
            .sections
            .iter()
            .chain(&base.sections)
            .filter(|section| section.enabled == Enabled::No)
            .map(|section| section.name.clone())
            .collect();
        // override_department は true と区別する。さらに上の継承で先頭へ移す条件が変わる。
        for section in &mut self.sections {
            if section
                .name
                .rsplit_once('/')
                .is_some_and(|(department, _)| disabled.contains(department))
                && section.enabled.truthy()
            {
                section.enabled = Enabled::Other;
            }
        }
        let mut prepend = Vec::new();
        for department in self
            .sections
            .iter()
            .filter(|section| section.enabled == Enabled::No)
        {
            let prefix = format!("{}/", department.name);
            prepend.extend(
                base.sections
                    .iter()
                    .filter(|section| {
                        section.name.starts_with(&prefix) && section.enabled == Enabled::Yes
                    })
                    .map(|section| section.name.clone()),
            );
        }
        // 本家は singleton Hash を手前へ merge し、既存 cop の位置も移す。
        for name in prepend {
            let mut section = if let Some(index) = self
                .sections
                .iter()
                .position(|section| section.name == name)
            {
                self.sections.remove(index)
            } else {
                SectionKeys {
                    name,
                    parameters: Some(Vec::new()),
                    enabled: Enabled::Missing,
                }
            };
            if let Some(parameters) = &mut section.parameters {
                let enabled = Value::String("Enabled".to_owned());
                parameters.retain(|parameter| parameter != &enabled);
                parameters.insert(0, enabled);
                if section.enabled == Enabled::Missing {
                    section.enabled = Enabled::No;
                }
            }
            self.sections.insert(0, section);
        }
        for inherited in &base.sections {
            let Some(base_keys) = &inherited.parameters else {
                continue;
            };
            if let Some(current) = self
                .sections
                .iter_mut()
                .find(|section| section.name == inherited.name)
            {
                if let Some(parameters) = &mut current.parameters {
                    let mut keys = base_keys.clone();
                    for key in parameters.iter() {
                        if !keys.contains(key) {
                            keys.push(key.clone());
                        }
                    }
                    *parameters = keys;
                    if current.enabled == Enabled::Missing {
                        current.enabled = inherited.enabled;
                    }
                }
            } else {
                self.sections.push(inherited.clone());
            }
        }
    }

    pub(super) fn names(&self) -> Vec<String> {
        self.sections
            .iter()
            .map(|section| section.name.clone())
            .collect()
    }

    pub(super) fn rename(&mut self, old: &str, new: &str) {
        let index = self
            .sections
            .iter()
            .position(|section| section.name == old)
            .unwrap();
        let mut source = self.sections.remove(index);
        source.name = new.to_owned();
        // Hash#[]= は衝突先の slot を保ち、parameter 順や Enabled も節ごと置き換える。
        if let Some(target) = self.sections.iter_mut().find(|section| section.name == new) {
            *target = source;
        } else {
            self.sections.push(source);
        }
    }

    fn prepare(
        &self,
        value: &Value,
        display: &str,
        target: &super::obsoletion::TargetRuby,
        registry: &Snapshot,
    ) -> Prepared {
        // 今回移行するwarning旧名以外は、従来の廃止名拒否も維持する。
        let mut legacy_unknown: Vec<_> = super::cop_names(value)
            .into_iter()
            .filter(|name| {
                !SCHEMA.contains_key(name)
                    && super::namespaces::is_obsolete(name)
                    && !super::obsoletion::warning_name(name)
            })
            .collect();
        legacy_unknown.sort();
        let unknown_names: HashSet<_> = super::unrecognized::names_with_registry(value, registry)
            .into_iter()
            .collect();
        let ordered_names: Vec<_> = self
            .sections
            .iter()
            .filter(|section| unknown_names.contains(&section.name))
            .map(|section| section.name.clone())
            .collect();
        let unknown =
            super::unrecognized::messages_with_registry(&ordered_names, display, registry);
        let mut warnings = Vec::new();
        let known: Vec<_> = self
            .sections
            .iter()
            .filter(|section| SCHEMA.contains_key(&section.name))
            .collect();
        // 本家は全 section の形を検査してから parameter 名を検査する。
        if known.iter().all(|section| section.parameters.is_some()) {
            for section in known {
                let name = &section.name;
                let schema = &SCHEMA[name];
                for key in section.parameters.as_ref().unwrap() {
                    if key
                        .as_str()
                        .is_some_and(|key| COMMON.contains(&key) || schema.allowed.contains(key))
                    {
                        continue;
                    }
                    let parameter = parameter_name(key);
                    warnings.push(ConfigWarning::Parameter(format!(
                        "Warning: {name} does not support {parameter} parameter.\n\nSupported parameters are:\n\n  - {}\n",
                        schema.supported,
                    )));
                }
            }
        }
        Prepared {
            obsolete: super::obsoletion::Outcome::prepare(value, display, target, registry),
            new_cops: super::new_cops::validation_error(
                value,
                display,
                self.sections.iter().map(|section| section.name.as_str()),
                registry,
            ),
            warnings,
            unknown,
            legacy_unknown,
            namespaces: Vec::new(),
        }
    }
}

fn parameter_name(value: &Value) -> String {
    let mut value = value;
    while let Value::Tagged(tagged) = value {
        value = &tagged.value;
    }
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => serde_yaml_ng::to_string(value)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    }
}

/// Namespace は Registry が常に plain で出し、parameter は CLI phase の色に従う。
#[derive(Debug)]
pub(crate) enum ConfigWarning {
    Namespace(String),
    Parameter(String),
    Obsoletion(String),
    CacheRoot(String),
    Unrecognized(Vec<String>),
}

#[derive(Debug, Default)]
struct Prepared {
    obsolete: super::obsoletion::Outcome,
    new_cops: Option<super::new_cops::InvalidNewCopsError>,
    namespaces: Vec<ConfigWarning>,
    warnings: Vec<ConfigWarning>,
    unknown: Vec<String>,
    legacy_unknown: Vec<String>,
}

/// memo の辺をたどるたび再生する。平坦な履歴を複製すると diamond で保存量が膨らむ。
#[derive(Debug, Default)]
pub(super) struct ValidationTrace {
    inherited: Vec<Arc<Self>>,
    own: Arc<Prepared>,
    has_warnings: bool,
    has_unknown: bool,
    has_obsolete: bool,
    has_new_cops: bool,
}

impl ValidationTrace {
    pub(super) fn document(
        inherited: Vec<Arc<Self>>,
        keys: &OrderedKeys,
        value: &Value,
        namespaces: Vec<ConfigWarning>,
        display: &str,
        target: &super::obsoletion::TargetRuby,
        registry: &Snapshot,
    ) -> Arc<Self> {
        let mut own = keys.prepare(value, display, target, registry);
        own.namespaces = namespaces;
        Self::new(inherited, own)
    }

    pub(super) fn namespaces(warnings: Vec<ConfigWarning>) -> Arc<Self> {
        Self::new(
            Vec::new(),
            Prepared {
                namespaces: warnings,
                ..Prepared::default()
            },
        )
    }

    pub(super) fn sequence(inherited: Vec<Arc<Self>>) -> Arc<Self> {
        Self::new(inherited, Prepared::default())
    }

    pub(super) fn with_children(&self, inherited: Vec<Arc<Self>>) -> Arc<Self> {
        Self::shared(inherited, Arc::clone(&self.own))
    }

    fn new(inherited: Vec<Arc<Self>>, own: Prepared) -> Arc<Self> {
        Self::shared(inherited, Arc::new(own))
    }

    fn shared(inherited: Vec<Arc<Self>>, own: Arc<Prepared>) -> Arc<Self> {
        Arc::new(Self {
            has_obsolete: own.obsolete.is_fatal()
                || inherited.iter().any(|trace| trace.has_obsolete),
            has_new_cops: own.new_cops.is_some()
                || inherited.iter().any(|trace| trace.has_new_cops),
            has_warnings: !own.unknown.is_empty()
                || own.obsolete.warning.is_some()
                || !own.namespaces.is_empty()
                || !own.warnings.is_empty()
                || inherited.iter().any(|trace| trace.has_warnings),
            has_unknown: !own.unknown.is_empty()
                || !own.legacy_unknown.is_empty()
                || inherited.iter().any(|trace| trace.has_unknown),
            inherited,
            own,
        })
    }

    pub(super) fn check_new_cops(&self) -> Result<()> {
        // quiet APIも継承先から検査する。妥当な共有部分木を展開しない。
        let mut stack = vec![(self, false)];
        while let Some((trace, own)) = stack.pop() {
            if !trace.has_new_cops {
                continue;
            }
            if own {
                if let Some(error) = &trace.own.new_cops {
                    return Err(error.clone().into());
                }
            } else {
                stack.push((trace, true));
                stack.extend(
                    trace
                        .inherited
                        .iter()
                        .rev()
                        .map(|child| (child.as_ref(), false)),
                );
            }
        }
        Ok(())
    }

    fn replay(
        &self,
        ignore_unknown: bool,
        emit: &(dyn Fn(&ConfigWarning) -> Result<()> + Send + Sync),
    ) -> Result<()> {
        enum Visit<'a> {
            Tree(&'a ValidationTrace),
            Own(&'a Prepared),
        }
        let mut stack = vec![Visit::Tree(self)];
        while let Some(visit) = stack.pop() {
            match visit {
                Visit::Tree(trace) => {
                    // 警告もfatalもない共有部分木だけを省略し、ignoreで廃止copの拒否を失わない。
                    if !trace.has_obsolete
                        && !trace.has_new_cops
                        && !trace.has_warnings
                        && (ignore_unknown || !trace.has_unknown)
                    {
                        continue;
                    }
                    stack.push(Visit::Own(&trace.own));
                    stack.extend(trace.inherited.iter().rev().map(|child| Visit::Tree(child)));
                }
                Visit::Own(prepared) => {
                    for warning in &prepared.namespaces {
                        emit(warning)?;
                    }
                    // 評価失敗/廃止項目のfatalはignoreの対象ではなく、unknownより先に拒否する。
                    prepared.obsolete.check()?;
                    if let Some(warning) = &prepared.obsolete.warning {
                        emit(&ConfigWarning::Obsoletion(warning.clone()))?;
                    }
                    if !ignore_unknown && !prepared.legacy_unknown.is_empty() {
                        bail!(
                            "unrecognized cop(s): {}",
                            prepared.legacy_unknown.join(", ")
                        );
                    }
                    if !prepared.unknown.is_empty() {
                        if ignore_unknown {
                            emit(&ConfigWarning::Unrecognized(prepared.unknown.clone()))?;
                        } else {
                            return Err(super::UnknownConfigError::new(&prepared.unknown).into());
                        }
                    }
                    if let Some(error) = &prepared.new_cops {
                        return Err(error.clone().into());
                    }
                    for warning in &prepared.warnings {
                        emit(warning)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct ValidationBundle {
    pub(super) origin: Option<PathBuf>,
    pub(super) trace: Arc<ValidationTrace>,
    pub(super) documents: HashMap<PathBuf, Arc<ValidationTrace>>,
}

type WarningOutput = dyn Fn(&ConfigWarning) -> Result<()> + Send + Sync;

/// library の値解決と CLI の警告出力を分ける。parameter の色は出力時の CLI phase が決める。
#[derive(Clone)]
pub(crate) struct ConfigWarnings {
    pub(super) cache_root_override: Option<PathBuf>,
    pub(super) remote_cache_root: Arc<std::sync::Mutex<Option<String>>>,
    pub(super) runtime: Arc<super::RuntimeSession>,
    pub(super) ignore_unknown: bool,
    pub(super) parent_exclusion: bool,
    output: Arc<WarningOutput>,
}

impl fmt::Debug for ConfigWarnings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigWarnings")
            .field("ignore_unknown", &self.ignore_unknown)
            .field("parent_exclusion", &self.parent_exclusion)
            .finish_non_exhaustive()
    }
}

// formatter/警告callback自身のI/O失敗を、設定拒否としてfinishedへ再送しない。
#[derive(Debug)]
pub(crate) struct WarningOutputFailure(anyhow::Error);

impl std::fmt::Display for WarningOutputFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(output)
    }
}

impl std::error::Error for WarningOutputFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl ConfigWarnings {
    pub(crate) fn with_cache_root(mut self, root: Option<PathBuf>) -> Self {
        self.cache_root_override = root;
        self
    }
    pub(crate) fn emit_cache_root(&self, message: String) -> Result<()> {
        (self.output)(&ConfigWarning::CacheRoot(message))
    }

    pub(crate) fn new(
        ignore_unknown: bool,
        parent_exclusion: bool,
        output: impl Fn(&ConfigWarning) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            cache_root_override: None,
            remote_cache_root: Arc::new(std::sync::Mutex::new(None)),
            runtime: Arc::new(super::RuntimeSession::default()),
            ignore_unknown,
            parent_exclusion,
            output: Arc::new(move |warning| {
                output(warning).map_err(|error| WarningOutputFailure(error).into())
            }),
        }
    }

    pub(crate) fn with_runtime(mut self, runtime: Arc<super::RuntimeSession>) -> Self {
        self.runtime = runtime;
        self
    }

    pub(super) fn emit(&self, trace: &ValidationTrace) -> Result<()> {
        trace.replay(self.ignore_unknown, self.output.as_ref())
    }

    pub(super) fn emit_using(&self, message: &str) -> Result<()> {
        // Config#initializeのheredocは本文内にLFを持ち、既存Parameterと同じreset境界になる。
        (self.output)(&ConfigWarning::Parameter(message.to_owned()))
    }
}
