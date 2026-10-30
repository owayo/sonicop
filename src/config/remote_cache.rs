use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use super::loader::config_io_error;

const CACHE_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_REMOTE_CONFIG_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Default)]
struct RemoteResolver(DefaultResolver);

impl Resolver for RemoteResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> std::result::Result<ResolvedSocketAddrs, ureq::Error> {
        self.0.resolve(uri, config, timeout).map_err(|error| {
            // getaddrinfo の I/O と、接続後の I/O を区別して SocketError だけを救済する。
            match error {
                ureq::Error::Io(_) => ureq::Error::HostNotFound,
                other => other,
            }
        })
    }
}

pub(super) fn read(url: &str, path: &Path) -> Result<String> {
    let modified = fs::metadata(path)
        .ok()
        .and_then(|info| info.modified().ok());
    if expired(modified, SystemTime::now()) {
        update(url, path, modified)?;
    }
    let contents = fs::read_to_string(path).map_err(|error| config_io_error(path, error))?;
    // File.write / File.read は Windows でそれぞれ一度だけ text mode を適用する。
    #[cfg(windows)]
    let contents = contents.replace("\r\n", "\n");
    Ok(contents)
}

fn expired(modified: Option<SystemTime>, now: SystemTime) -> bool {
    modified.is_none_or(|mtime| {
        now.duration_since(mtime)
            .is_ok_and(|age| age > CACHE_LIFETIME)
    })
}

fn update(url: &str, path: &Path, modified: Option<SystemTime>) -> Result<()> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build();
    let agent = ureq::Agent::with_parts(
        config,
        DefaultConnector::default(),
        RemoteResolver::default(),
    );
    let mut request = agent.get(url);
    if let Some(mtime) = modified {
        // Time#rfc2822 は GMT 固定ではなく、保存時刻のローカル UTC offset を出す。
        let date = DateTime::<Local>::from(mtime)
            .format("%a, %d %b %Y %H:%M:%S %z")
            .to_string();
        request = request.header("If-Modified-Since", date);
    }
    let mut response = match request.call() {
        Ok(response) => response,
        // 本家の SocketError 救済に対応する。接続拒否や timeout へは広げない。
        Err(ureq::Error::HostNotFound) => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to fetch remote configuration: {url}"));
        }
    };
    // 304 は mtime を進めないため、古い cache は次の process でも再検証される。
    if response.status() == 304 {
        return Ok(());
    }
    let contents = response
        .body_mut()
        .with_config()
        // ureq は指定値と同じ長さも拒否するので、論理上限の一つ先を渡す。
        .limit(MAX_REMOTE_CONFIG_BYTES + 1)
        .read_to_string()
        .with_context(|| format!("failed to read remote configuration: {url}"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| config_io_error(parent, error))?;
    }
    #[cfg(windows)]
    let contents = contents.replace('\n', "\r\n");
    // rename で読み取り専用 cache を置換せず、File.write と同じ OS の判定に従う。
    fs::write(path, contents).map_err(|error| config_io_error(path, error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_only_after_the_full_twenty_four_hours() {
        let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800);
        // RuboCop 1.89.0 RemoteConfig の実 API で、境界ぴったりは取得しない。
        assert!(!expired(Some(mtime), mtime + CACHE_LIFETIME));
        assert!(expired(
            Some(mtime),
            mtime + CACHE_LIFETIME + Duration::from_secs(1)
        ));
        assert!(!expired(Some(mtime), mtime - Duration::from_secs(1)));
        assert!(expired(None, mtime));
    }
}
