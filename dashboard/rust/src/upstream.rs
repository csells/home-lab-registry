use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use axum::http::Uri;
use reqwest::{Certificate, Client, Response, Url, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

fn parse_origin(value: &str) -> Result<Url, &'static str> {
    let uri: Uri = value.parse().map_err(|_| "Invalid upstream address")?;
    let url = Url::parse(value).map_err(|_| "Invalid upstream URL")?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || uri.path() != "/"
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
    {
        return Err("An HTTPS upstream origin is required");
    }
    Ok(url)
}

pub(crate) fn lease_query(raw: &str) -> Result<(String, Option<String>), &'static str> {
    let mut scope = None;
    let mut before = None;
    for (key, value) in form_urlencoded::parse(raw.as_bytes()) {
        match key.as_ref() {
            "scope"
                if scope.is_none()
                    && matches!(value.as_ref(), "all" | "current" | "released" | "counts") =>
            {
                scope = Some(value.into_owned());
            }
            "before"
                if before.is_none()
                    && !value.is_empty()
                    && value.len() <= 18
                    && !value.starts_with('0')
                    && value.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                before = Some(value.into_owned());
            }
            _ => return Err("Invalid lease query"),
        }
    }
    let scope = scope.unwrap_or_else(|| "all".to_owned());
    if scope == "counts" && before.is_some() {
        return Err("Counts cannot be paginated");
    }
    Ok((scope, before))
}

pub(crate) async fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, &'static str> {
    let content = tokio::fs::read(path)
        .await
        .map_err(|_| "Configuration is unavailable")?;
    serde_json::from_slice(&content).map_err(|_| "Configuration is invalid")
}

#[derive(Clone)]
pub(crate) struct SecureClient {
    origin: Url,
    client: Client,
    token: String,
}

impl SecureClient {
    pub(crate) async fn new(
        origin: &str,
        ca: &Path,
        token: String,
        read_timeout: Duration,
        allow_proxy: bool,
    ) -> Result<Self, &'static str> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let origin = parse_origin(origin)?;
        let certificate = tokio::fs::read(ca)
            .await
            .map_err(|_| "Upstream CA unavailable")?;
        let roots =
            Certificate::from_pem_bundle(&certificate).map_err(|_| "Upstream CA is invalid")?;
        if token.is_empty() || roots.is_empty() {
            return Err("Incomplete upstream credentials");
        }
        let mut builder = Client::builder()
            .https_only(true)
            .redirect(Policy::none())
            .referer(false)
            .tls_certs_only(roots)
            .connect_timeout(read_timeout.min(Duration::from_secs(10)))
            .read_timeout(read_timeout);
        if !allow_proxy {
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|_| "Cannot initialize upstream client")?;
        Ok(Self {
            origin,
            client,
            token,
        })
    }

    pub(crate) async fn open(
        &self,
        path: &'static str,
        query: &[(&str, &str)],
        accept: &'static str,
        last: Option<&str>,
    ) -> Result<Response, &'static str> {
        let mut url = self.origin.clone();
        url.set_path(path);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query.iter().copied());
        }
        let mut request = self
            .client
            .get(url)
            .bearer_auth(&self.token)
            .header("Accept", accept)
            .header("User-Agent", "HomeLabRegistry/1");
        if let Some(last) = last {
            request = request.header("Last-Event-ID", last);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Upstream request interrupted")?;
        if !response.status().is_success() {
            return Err("Upstream rejected request");
        }
        Ok(response)
    }

    pub(crate) async fn json(
        &self,
        path: &'static str,
        query: &[(&str, &str)],
        limit: usize,
    ) -> Result<Value, &'static str> {
        let mut response = self.open(path, query, "application/json", None).await?;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Incomplete upstream response")?
        {
            if chunk.len() > limit - body.len() {
                return Err("Upstream response exceeded size limit");
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| "Invalid upstream JSON")
    }
}

#[derive(Deserialize)]
struct LeaseSettings {
    endpoint: String,
    ca_file: PathBuf,
    token_file: PathBuf,
}

pub(crate) async fn lease_page(
    config: &Path,
    scope: &str,
    cursor: Option<&str>,
) -> Result<Value, &'static str> {
    let config: LeaseSettings = read_json(config).await?;
    let token = tokio::fs::read_to_string(config.token_file)
        .await
        .map_err(|_| "Lease token unavailable")?;
    let client = SecureClient::new(
        &config.endpoint,
        &config.ca_file,
        token.trim().into(),
        Duration::from_secs(8),
        false,
    )
    .await?;
    let mut query = vec![("scope", scope)];
    if let Some(cursor) = cursor {
        query.push(("before", cursor));
    }
    tokio::time::timeout(
        Duration::from_secs(8),
        client.json("/v1/lease-review", &query, 262_144),
    )
    .await
    .map_err(|_| "Lease review timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_authority_is_validated_before_configured_credentials_can_leave() {
        let origin = parse_origin("https://registry.example.invalid/").unwrap();
        assert_eq!(origin.as_str(), "https://registry.example.invalid/");
        for value in [
            "http://registry.example.invalid/",
            "https://user@registry.example.invalid/",
            "https://registry.example.invalid/other",
            "https://registry.example.invalid/?query",
            "https://registry.example.invalid/#fragment",
        ] {
            assert!(parse_origin(value).is_err(), "{value}");
        }
    }

    #[test]
    fn lease_paging_rejects_duplicate_parameters_and_unscoped_internal_queries() {
        assert_eq!(
            lease_query("scope=released&before=40").unwrap(),
            ("released".into(), Some("40".into()))
        );
        assert_eq!(lease_query("").unwrap(), ("all".into(), None));
        for bad in [
            "scope=inventory",
            "scope=counts&before=3",
            "before=0",
            "before=02",
            "before=22&before=30",
            "scope=all&scope=all",
            "scope=all&other=1",
            "before=%E2%98%83",
        ] {
            assert!(lease_query(bad).is_err(), "{bad}");
        }
    }
}
