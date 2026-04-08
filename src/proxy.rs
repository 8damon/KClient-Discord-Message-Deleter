use std::{fs, path::Path};

use anyhow::{Context, Result};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT},
    Client, Proxy,
};

pub struct ProxyEntry {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

pub fn load_proxy_file(path: &Path) -> Result<Vec<ProxyEntry>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_proxy_list(&content)
}

pub fn parse_proxy_list(content: &str) -> Result<Vec<ProxyEntry>> {
    let mut proxies = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        anyhow::ensure!(
            parts.len() == 4,
            "proxy.txt line {}: expected ip:port:username:password, got {:?}",
            i + 1,
            line
        );
        let port = parts[1]
            .parse::<u16>()
            .with_context(|| format!("proxy.txt line {}: invalid port {:?}", i + 1, parts[1]))?;
        proxies.push(ProxyEntry {
            host: parts[0].to_string(),
            port,
            username: parts[2].to_string(),
            password: parts[3].to_string(),
        });
    }
    Ok(proxies)
}

pub fn build_client_pool(token: &str, proxies: &[ProxyEntry]) -> Result<Vec<Client>> {
    let headers = build_headers(token)?;
    let mut clients = Vec::with_capacity(proxies.len() + 1);

    clients.push(
        Client::builder()
            .default_headers(headers.clone())
            .build()
            .context("failed to build direct HTTP client")?,
    );

    for entry in proxies {
        let url = format!(
            "http://{}:{}@{}:{}",
            percent_encode(&entry.username),
            percent_encode(&entry.password),
            entry.host,
            entry.port,
        );
        let proxy = Proxy::all(&url)
            .with_context(|| format!("invalid proxy address {}:{}", entry.host, entry.port))?;

        clients.push(
            Client::builder()
                .default_headers(headers.clone())
                .proxy(proxy)
                .build()
                .with_context(|| {
                    format!(
                        "failed to build proxy client for {}:{}",
                        entry.host, entry.port
                    )
                })?,
        );
    }

    Ok(clients)
}

pub fn build_direct_client(token: &str) -> Result<Client> {
    let headers = build_headers(token)?;
    Client::builder()
        .default_headers(headers)
        .build()
        .context("failed to build HTTP client")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'!'
            | b'\''
            | b'('
            | b')'
            | b'*' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(
                    char::from_digit((byte >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((byte & 0xf) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

fn build_headers(token: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    let mut auth =
        HeaderValue::from_str(token).context("TOKEN contains invalid header characters")?;
    auth.set_sensitive(true);
    headers.insert(AUTHORIZATION, auth);
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("kcordclient/0.1 (+https://discord.com)"),
    );
    Ok(headers)
}
