//! RuboCop 1.89.0 と同じ rubydex コアを、一つの実行の cop 間で共有する。

use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use rubydex::indexing::{IndexerBackend, index_files};
use rubydex::listing::collect_file_paths;
use rubydex::model::declaration::{Ancestor, Declaration};
use rubydex::model::definitions::{Definition, Mixin};
use rubydex::model::graph::Graph;
use rubydex::model::ids::{ConstantReferenceId, NameId, StringId, declaration_id_from_lookup_name};
use rubydex::model::name::{NameRef, ParentScope};
use rubydex::model::visibility::Visibility;
use rubydex::resolution::Resolver;

use crate::config::{Config, ConfigStore};

pub(crate) use rubydex::model::ids::DeclarationId;

/// 実行開始時のプロジェクト索引。プロジェクト内の解析は Rust だけで完結する。
pub struct ProjectIndex {
    // rubydex の定数解決は一時的な名前を登録するため、読み取りの問い合わせにも排他が要る。
    graph: Mutex<Graph>,
    reference_names: HashSet<String>,
}

impl fmt::Debug for ProjectIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectIndex")
            .finish_non_exhaustive()
    }
}

impl ProjectIndex {
    pub(crate) fn enabled(config: &Config) -> bool {
        config
            .all_cops_value::<serde_yaml_ng::Value>("UseProjectIndex")
            .is_some_and(|value| {
                !matches!(
                    value,
                    serde_yaml_ng::Value::Null | serde_yaml_ng::Value::Bool(false)
                )
            })
    }

    pub(crate) fn build(targets: &[PathBuf], configs: &ConfigStore) -> Result<Self> {
        let config = configs.root();
        // Runner#project_index_files は単一ファイル指定でも設定の基準ディレクトリ全体を索引化する。
        let mut paths = crate::engine::discover_targets_with_store(
            &[config.path_base().to_path_buf()],
            config.cwd(),
            configs,
            false,
            false,
        )?;
        paths.extend(targets.iter().map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                config.cwd().join(path)
            }
        }));
        if config
            .all_cops_value::<serde_yaml_ng::Value>("ProjectIndexIncludesGems")
            .is_some_and(|value| {
                !matches!(
                    value,
                    serde_yaml_ng::Value::Null | serde_yaml_ng::Value::Bool(false)
                )
            })
        {
            paths.extend(bundled_gem_files(config.cwd()));
        }
        paths.sort();
        paths.dedup();
        let mut graph = Graph::new();
        // index_all は明示指定したファイルにも拡張子の制限を適用する。
        // 返された I/O エラーは本家の ProjectIndexLoader も使用せず、残りの索引で続行する。
        let (paths, _) = collect_file_paths(
            paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            &graph.excluded_patterns(),
        );
        let _ = index_files(&mut graph, paths, IndexerBackend::RubyIndexer);
        Resolver::new(&mut graph).resolve();
        let reference_names = graph
            .method_references()
            .values()
            .filter_map(|reference| {
                graph
                    .strings()
                    .get(reference.str())
                    .map(|name| name.strip_suffix("()").unwrap_or(name).to_owned())
            })
            .collect();
        Ok(Self {
            graph: Mutex::new(graph),
            reference_names,
        })
    }

    pub(crate) fn with<R>(&self, query: impl FnOnce(&mut Index<'_>) -> R) -> R {
        let mut graph = self.graph.lock().expect("project index query panicked");
        query(&mut Index {
            graph: &mut graph,
            reference_names: &self.reference_names,
        })
    }
}

fn bundled_gem_files(cwd: &std::path::Path) -> Vec<PathBuf> {
    let Some(ruby) = std::env::var_os("SONICOP_BUNDLER_RUBY") else {
        return Vec::new();
    };
    // gem の解決と glob は同じ Ruby/Bundler に委ねる。通常の起動ではこのプロセスを作らない。
    const SCRIPT: &str = r#"
require 'json'
begin
  gem 'bundler', ENV.fetch('SONICOP_BUNDLER_VERSION') if ENV['SONICOP_BUNDLER_VERSION']
  require 'bundler'
  files = if defined?(Bundler) && Bundler::SharedHelpers.in_bundle?
    Bundler.load.specs.flat_map { |spec| Dir.glob(File.join(spec.full_gem_path, 'lib', '**', '*.rb')) }
  else
    []
  end
  result = { files: files }
rescue StandardError => error
  result = { files: [], error: error.message }
end
STDOUT.write("\0sonicop-bundle-index\0", JSON.generate(result))
"#;
    const MARKER: &[u8] = b"\0sonicop-bundle-index\0";
    let result = (|| -> Result<Vec<PathBuf>> {
        let output = std::process::Command::new(ruby)
            .args(["-e", SCRIPT])
            .current_dir(cwd)
            .output()?;
        let start = output
            .stdout
            .windows(MARKER.len())
            .rposition(|window| window == MARKER)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Ruby bundle query did not return a report ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                )
            })?;
        // Bundler が依存解決の進捗を出す場合、本家 CLI と同じ出力先へ戻す。
        // 機械可読の応答だけをマーカーの後ろに置き、進捗を JSON として解釈しない。
        use std::io::Write;
        std::io::stdout().write_all(&output.stdout[..start])?;
        std::io::stderr().write_all(&output.stderr)?;
        #[derive(serde::Deserialize)]
        struct Report {
            files: Vec<PathBuf>,
            error: Option<String>,
        }
        let report: Report = serde_json::from_slice(&output.stdout[start + MARKER.len()..])?;
        if let Some(error) = report.error {
            anyhow::bail!("{error}");
        }
        if !output.status.success() {
            anyhow::bail!("Ruby bundle query failed ({})", output.status);
        }
        Ok(report.files)
    })();
    result.unwrap_or_else(|error| {
        eprintln!("rubydex bundle indexing failed: {error}. Indexing the project only.");
        Vec::new()
    })
}

pub(crate) struct Index<'a> {
    graph: &'a mut Graph,
    reference_names: &'a HashSet<String>,
}

impl Index<'_> {
    pub(crate) fn lookup(&self, name: &str) -> Option<DeclarationId> {
        let id = declaration_id_from_lookup_name(name);
        self.graph.declarations().contains_key(&id).then_some(id)
    }

    pub(crate) fn name(&self, id: DeclarationId) -> &str {
        self.graph
            .declarations()
            .get(&id)
            .map_or("", Declaration::name)
    }

    pub(crate) fn namespace(&self, id: DeclarationId) -> bool {
        self.graph
            .declarations()
            .get(&id)
            .is_some_and(|declaration| declaration.as_namespace().is_some())
    }

    pub(crate) fn resolve_constant(
        &mut self,
        name: &str,
        nesting: &[String],
    ) -> Option<DeclarationId> {
        // rubydex-sys/name_api.rs と同じ名前登録と参照数の復元を行う。
        let mut scope = None;
        let mut names = Vec::new();
        for entry in nesting {
            scope = self.qualified_name(entry, scope, &mut names);
        }
        let name_id = self.qualified_name(name, scope, &mut names);
        let result = name_id.and_then(|id| Resolver::new(self.graph).resolve_constant(id));
        for id in names {
            self.graph.untrack_name(id);
        }
        result
    }

    fn qualified_name(
        &mut self,
        name: &str,
        nesting: Option<NameId>,
        temporary: &mut Vec<NameId>,
    ) -> Option<NameId> {
        let mut parent = ParentScope::None;
        for part in name.split("::") {
            if part.is_empty() {
                parent = ParentScope::TopLevel;
                continue;
            }
            let string = self.graph.intern_string(part.to_owned());
            let id = self.graph.add_name(string, parent, nesting);
            temporary.push(id);
            parent = ParentScope::Some(id);
        }
        match parent {
            ParentScope::Some(id) | ParentScope::Attached(id) => Some(id),
            _ => None,
        }
    }

    pub(crate) fn find_member(&self, owner: DeclarationId, name: &str) -> Option<DeclarationId> {
        rubydex::query::find_member_in_ancestors(self.graph, owner, StringId::from(name), false)
            .ok()
    }

    pub(crate) fn member(&self, owner: DeclarationId, name: &str) -> Option<DeclarationId> {
        self.graph
            .declarations()
            .get(&owner)?
            .as_namespace()?
            .member(&StringId::from(name))
            .copied()
    }

    fn ancestors(&self, owner: DeclarationId) -> Vec<DeclarationId> {
        self.graph
            .declarations()
            .get(&owner)
            .and_then(Declaration::as_namespace)
            .map_or_else(Vec::new, |namespace| {
                namespace
                    .ancestors()
                    .into_iter()
                    .filter_map(|ancestor| match ancestor {
                        Ancestor::Complete(id) => Some(*id),
                        Ancestor::Partial(_) => None,
                    })
                    .collect()
            })
    }

    pub(crate) fn singleton(&self, owner: DeclarationId) -> Option<DeclarationId> {
        let name = self.name(owner);
        self.lookup(&format!("{name}::<{}>", name.rsplit("::").next()?))
    }

    pub(crate) fn singleton_member(
        &self,
        owner: DeclarationId,
        name: &str,
    ) -> Option<DeclarationId> {
        // 最初に見つかった特異クラスの探索結果を返す。nil でも次の祖先へ進まない。
        for ancestor in self.ancestors(owner) {
            if let Some(singleton) = self.singleton(ancestor) {
                return self.find_member(singleton, name);
            }
        }
        None
    }

    pub(crate) fn deprecated_detail(&self, id: DeclarationId) -> Option<String> {
        let definitions = self.graph.declarations().get(&id)?.definitions();
        if definitions.is_empty()
            || !definitions.iter().all(|id| {
                self.graph
                    .definitions()
                    .get(id)
                    .is_some_and(Definition::is_deprecated)
            })
        {
            return None;
        }
        let comment = definitions
            .iter()
            .filter_map(|id| self.graph.definitions().get(id))
            .find_map(|definition| {
                definition
                    .comments()
                    .iter()
                    .find(|comment| comment.string().contains("@deprecated"))
            });
        let detail = comment
            .and_then(|comment| comment.string().split_once("@deprecated"))
            .and_then(|(_, tail)| {
                // Ruby の \s+ は改行も含むが、続く .+ は改行を含まない。
                let text =
                    tail.trim_start_matches(|character: char| character.is_ascii_whitespace());
                (tail.starts_with(|character: char| character.is_ascii_whitespace())
                    && !text.is_empty())
                .then(|| text.lines().next().unwrap_or("").trim())
            })
            .filter(|detail| !detail.is_empty());
        Some(match detail {
            None => ".".to_owned(),
            Some(text) if text.ends_with('.') => format!(": {text}"),
            Some(text) => format!(": {text}."),
        })
    }

    pub(crate) fn constant_names(&self, owner: DeclarationId) -> Vec<String> {
        self.members(owner)
            .into_iter()
            .filter_map(|id| {
                let name = self.name(id);
                (!name.contains(['#', '<']))
                    .then(|| name.rsplit("::").next().unwrap_or(name).to_owned())
            })
            .collect()
    }

    fn members(&self, owner: DeclarationId) -> Vec<DeclarationId> {
        self.graph
            .declarations()
            .get(&owner)
            .and_then(Declaration::as_namespace)
            .map_or_else(Vec::new, |namespace| {
                namespace.members().values().copied().collect()
            })
    }

    pub(crate) fn method_names(&self, owner: DeclarationId) -> Vec<String> {
        let mut scopes: Vec<_> = self
            .ancestors(owner)
            .into_iter()
            .filter_map(|id| self.singleton(id))
            .collect();
        scopes.push(owner);
        scopes
            .into_iter()
            .flat_map(|scope| self.members(scope))
            .filter_map(|id| {
                let name = self.name(id).rsplit_once('#')?.1.strip_suffix("()")?;
                method_identifier(name).then(|| name.to_owned())
            })
            .collect()
    }

    pub(crate) fn complete_ancestry(&self, owner: DeclarationId) -> bool {
        self.complete_ancestry_with(owner, false)
    }

    fn complete_ancestry_with(&self, owner: DeclarationId, ignore_extend: bool) -> bool {
        self.ancestors(owner).into_iter().all(|ancestor| {
            self.graph
                .declarations()
                .get(&ancestor)
                .is_none_or(|declaration| {
                    declaration.definitions().iter().all(|id| {
                        let Some(definition) = self.graph.definitions().get(id) else {
                            return true;
                        };
                        let (superclass, mixins): (Option<&ConstantReferenceId>, &[Mixin]) =
                            match definition {
                                Definition::Class(definition) => {
                                    (definition.superclass_ref(), definition.mixins())
                                }
                                Definition::Module(definition) => (None, definition.mixins()),
                                Definition::SingletonClass(definition) => {
                                    (None, definition.mixins())
                                }
                                _ => (None, &[]),
                            };
                        superclass
                            .into_iter()
                            .chain(
                                mixins
                                    .iter()
                                    .filter(|mixin| {
                                        !ignore_extend || !matches!(mixin, Mixin::Extend(_))
                                    })
                                    .map(Mixin::constant_reference_id),
                            )
                            .all(|id| {
                                self.graph
                                    .constant_references()
                                    .get(id)
                                    .is_none_or(|reference| {
                                        !matches!(
                                            self.graph.names().get(reference.name_id()),
                                            Some(NameRef::Unresolved(_))
                                        )
                                    })
                            })
                    })
                })
        })
    }

    pub(crate) fn namespace_kind(&self, owner: DeclarationId) -> Option<&'static str> {
        match self.graph.declarations().get(&owner)?.kind() {
            "Class" => Some("class"),
            "Module" => Some("module"),
            _ => None,
        }
    }

    pub(crate) fn has_descendants(&self, owner: DeclarationId) -> bool {
        self.graph
            .declarations()
            .get(&owner)
            .and_then(Declaration::as_namespace)
            .is_some_and(|namespace| namespace.descendants().iter().any(|id| *id != owner))
    }

    pub(crate) fn inherited_member(&self, owner: DeclarationId, member: &str) -> bool {
        self.ancestors(owner)
            .into_iter()
            .any(|ancestor| ancestor != owner && self.member(ancestor, member).is_some())
    }

    pub(crate) fn stateless_ancestry(&self, owner: DeclarationId) -> bool {
        self.namespace_kind(owner) == Some("class")
            && !self.inherited_member(owner, "initialize()")
            && self.complete_ancestry_with(owner, true)
    }

    pub(crate) fn exception_ancestor(&self, owner: DeclarationId) -> Option<String> {
        self.ancestors(owner).into_iter().find_map(|ancestor| {
            let declaration = self.graph.declarations().get(&ancestor)?;
            if declaration.kind() != "Class" {
                return None;
            }
            declaration
                .definitions()
                .iter()
                .any(|id| {
                    let Some(Definition::Class(definition)) = self.graph.definitions().get(id)
                    else {
                        return false;
                    };
                    definition
                        .superclass_ref()
                        .and_then(|id| self.graph.constant_references().get(id))
                        .and_then(|reference| self.graph.names().get(reference.name_id()))
                        .is_some_and(|name| {
                            // 本家の未解決参照の name は末尾だけを返す。Custom::Exception も同じ判定になる。
                            matches!(name, NameRef::Unresolved(_))
                                && self.graph.strings().get(name.str()).is_some_and(|name| {
                                    name.trim_start_matches("::") == "Exception"
                                })
                        })
                })
                .then(|| self.name(ancestor).to_owned())
        })
    }

    pub(crate) fn defined_elsewhere(
        &self,
        owner: DeclarationId,
        current: &std::path::Path,
        line: usize,
        documenting: bool,
    ) -> bool {
        self.graph
            .declarations()
            .get(&owner)
            .is_some_and(|declaration| {
                declaration.definitions().iter().any(|id| {
                    let Some(definition) = self.graph.definitions().get(id) else {
                        return false;
                    };
                    let Some(document) = self.graph.documents().get(definition.uri_id()) else {
                        return false;
                    };
                    let Some(path) = document.file_path() else {
                        return !documenting;
                    };
                    let same = same_file::is_same_file(&path, current).unwrap_or(false);
                    let definition_line =
                        definition.offset().to_location(document).start_line() as usize + 1;
                    (!same || definition_line != line)
                        && (!documenting
                            || definition.comments().iter().any(|comment| {
                                let text = comment.string().trim();
                                let body = text.strip_prefix('#').unwrap_or("").trim_start_matches(
                                    |character: char| character.is_ascii_whitespace(),
                                );
                                !text.is_empty()
                                    && ![
                                        "rubocop:",
                                        "frozen_string_literal:",
                                        "encoding:",
                                        "shareable_constant_value:",
                                        "typed:",
                                        "-*-",
                                    ]
                                    .iter()
                                    .any(|prefix| body.starts_with(prefix))
                            }))
                })
            })
    }

    fn definitions_in_other_files<'a>(
        &'a self,
        owner: DeclarationId,
        current: &std::path::Path,
    ) -> Vec<(&'a Definition, PathBuf, usize)> {
        let Some(declaration) = self.graph.declarations().get(&owner) else {
            return Vec::new();
        };
        let mut definitions: Vec<_> = declaration
            .definitions()
            .iter()
            .filter_map(|id| {
                let definition = self.graph.definitions().get(id)?;
                let document = self.graph.documents().get(definition.uri_id())?;
                let path = document.file_path()?;
                if same_file::is_same_file(&path, current).unwrap_or(false) {
                    return None;
                }
                let line = definition.offset().to_location(document).start_line() as usize + 1;
                Some((definition, path, line))
            })
            .collect();
        definitions.sort_by(|left, right| (&left.1, left.2).cmp(&(&right.1, right.2)));
        definitions
    }

    pub(crate) fn constant_in_other_file(
        &self,
        name: &str,
        current: &std::path::Path,
    ) -> Option<PathBuf> {
        let owner = self.lookup(name.trim_start_matches("::"))?;
        self.definitions_in_other_files(owner, current)
            .into_iter()
            .next()
            .map(|(_, path, _)| path)
    }

    pub(crate) fn method_in_other_file(
        &self,
        method: &str,
        current: &std::path::Path,
    ) -> Option<(PathBuf, usize)> {
        let separator = method.find(['#', '.'])?;
        let owner = &method[..separator];
        let name = &method[separator + 1..];
        // 本家の INDEXABLE_METHOD_NAME は定数パスの所有者だけを許す。
        if name.is_empty()
            || name.contains(['#', '.'])
            || !owner.split("::").all(|part| {
                part.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        {
            return None;
        }
        let namespace = if method.as_bytes()[separator] == b'.' {
            format!("{owner}::<{}>", owner.rsplit("::").next()?)
        } else {
            owner.to_owned()
        };
        let mut definitions = self
            .lookup(&format!("{namespace}#{name}()"))
            .map_or_else(Vec::new, |owner| {
                self.definitions_in_other_files(owner, current)
            });
        if let Some(reader) = name.strip_suffix('=')
            && !reader.is_empty()
            && reader
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            if let Some(owner) = self.lookup(&format!("{namespace}#{reader}()")) {
                definitions.extend(
                    self.definitions_in_other_files(owner, current)
                        .into_iter()
                        .filter(|(definition, _, _)| {
                            matches!(
                                definition,
                                Definition::AttrWriter(_) | Definition::AttrAccessor(_)
                            )
                        }),
                );
            }
        } else {
            definitions
                .retain(|(definition, _, _)| !matches!(definition, Definition::AttrWriter(_)));
        }
        definitions.sort_by(|left, right| (&left.1, left.2).cmp(&(&right.1, right.2)));
        // 別ファイルで alias と定義が同居すると、本家は意図的な再定義として扱う。
        let alias_paths: HashSet<_> = definitions
            .iter()
            .filter(|(definition, _, _)| matches!(definition, Definition::MethodAlias(_)))
            .map(|(_, path, _)| path)
            .collect();
        if definitions.iter().any(|(definition, path, _)| {
            !matches!(definition, Definition::MethodAlias(_)) && alias_paths.contains(path)
        }) {
            return None;
        }
        definitions
            .into_iter()
            .find(|(definition, _, _)| !matches!(definition, Definition::MethodAlias(_)))
            .map(|(_, path, line)| (path, line))
    }

    pub(crate) fn unused_private_method(&self, owner: DeclarationId, method: &str) -> bool {
        let member = format!("{method}()");
        let Some(declaration) = self.member(owner, &member) else {
            return false;
        };
        if self.graph.visibility(&declaration) != Some(Visibility::Private)
            || self.reference_names.contains(method)
        {
            return false;
        }
        let Some(namespace) = self
            .graph
            .declarations()
            .get(&owner)
            .and_then(Declaration::as_namespace)
        else {
            return false;
        };
        if namespace.descendants().iter().any(|id| *id != owner) {
            return false;
        }
        !self
            .ancestors(owner)
            .into_iter()
            .any(|ancestor| ancestor != owner && self.member(ancestor, &member).is_some())
    }
}

fn method_identifier(name: &str) -> bool {
    let name = name.strip_suffix(['?', '!']).unwrap_or(name);
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}
