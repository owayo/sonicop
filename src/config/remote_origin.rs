use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(super) fn path(url: &str, root: &str, cwd: &Path) -> Result<PathBuf> {
    let name = cache_name(url)?;
    let base = crate::formatter::absolute_path(Path::new(root), cwd);
    Ok(PathBuf::from(crate::formatter::absolute_path(
        Path::new(&name),
        Path::new(&base),
    )))
}

fn cache_name(url: &str) -> Result<String> {
    let (uri, path) = ruby_uri(url)?;
    // File.basenameは末尾slashを除くが、rootだけのpathにはslash自体を返す。
    let trimmed = path.trim_end_matches('/');
    let basename = if trimmed.is_empty() && !path.is_empty() {
        "/"
    } else {
        trimmed.rsplit('/').next().unwrap_or("")
    };
    let lower = basename.to_ascii_lowercase();
    let stem = if lower.ends_with(".yaml") {
        &basename[..basename.len() - 5]
    } else if lower.ends_with(".yml") {
        &basename[..basename.len() - 4]
    } else {
        basename
    };
    // byteslice(0,217).scrub('')は末尾の不完全なUTF-8を捨てる。
    let mut end = stem.len().min(217);
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    Ok(format!(
        "{}-{:x}.yml",
        &stem[..end],
        md5::compute(uri.as_bytes())
    ))
}

fn ruby_uri(url: &str) -> Result<(String, &str)> {
    let scheme_end = url
        .find("://")
        .context("remote configuration URL has no scheme")?;
    let scheme = url[..scheme_end].to_ascii_lowercase();
    let start = scheme_end + 3;
    let end = url[start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |i| start + i);
    let authority = &url[start..end];
    let normalized_authority = if let Some(colon) = authority.rfind(':')
        && authority.rfind('@').is_none_or(|at| colon > at)
        && authority.rfind(']').is_none_or(|bracket| colon > bracket)
        && let Ok(port) = authority[colon + 1..].parse::<u16>()
    {
        // URI::HTTP/HTTPS#to_sは既定portを除き、他のportは整数表記へ戻す。
        if (scheme.as_str(), port) == ("http", 80) || (scheme.as_str(), port) == ("https", 443) {
            authority[..colon].to_owned()
        } else {
            format!("{}:{port}", &authority[..colon])
        }
    } else {
        authority.to_owned()
    };
    let path_end = url[end..].find(['?', '#']).map_or(url.len(), |i| end + i);
    Ok((
        format!("{scheme}://{normalized_authority}{}", &url[end..]),
        &url[end..path_end],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_uri_spelling_match_twelve_real_remote_config_api_calls() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/remote_config_origins.json"
        ))
        .unwrap();
        assert_eq!(fixture["api"].as_array().unwrap().len(), 12);
        let folder = tempfile::tempdir().unwrap();
        for row in fixture["api"].as_array().unwrap() {
            let url = row["url"].as_str().unwrap();
            let (uri, uri_path) = ruby_uri(url).unwrap();
            assert_eq!(uri, row["uri"].as_str().unwrap(), "{url}");
            assert_eq!(uri_path, row["path"].as_str().unwrap(), "{url}");
            assert_eq!(
                cache_name(url).unwrap(),
                row["name"].as_str().unwrap(),
                "{url}"
            );
            #[cfg(windows)]
            {
                let cwd = folder.path().to_string_lossy().replace('\\', "/");
                let expected = row["cache_path_template"]
                    .as_str()
                    .unwrap()
                    .replace("<CWD_FWD>", &cwd)
                    .replace("<DRIVE>", &cwd[..2]);
                assert_eq!(
                    path(url, "cache/rubocop_cache", folder.path())
                        .unwrap()
                        .to_string_lossy(),
                    expected,
                    "{url}"
                );
            }
        }
        // Windows参照のcache pathをUnixの実測へ読み替えず、URI/nameだけを共通で照合する。
        drop(folder);
    }
}
