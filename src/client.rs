use std::time::Duration;

use anyhow::Context;
use base64::Engine;

use crate::cli::HttpOptions;

const UA_POOL: [&str; 6] = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:127.0) Gecko/20100101 Firefox/127.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36 Edg/125.0.0.0",
];

#[derive(Clone)]
pub struct ClientConfig {
    pub insecure: bool,
    pub proxy: Option<String>,
    pub timeout: u64,
    pub user_agent: String,
    pub headers: Vec<String>,
    pub basic: Option<String>,
    pub token: Option<String>,
    pub random_ua: bool,
    pub follow_redirects: bool,
    pub http2: bool,
    pub keepalive: bool,
    worker: usize,
}

impl ClientConfig {
    pub fn from_http(http: &HttpOptions) -> Self {
        ClientConfig {
            insecure: http.insecure,
            proxy: http.proxy.clone(),
            timeout: http.timeout,
            user_agent: http.user_agent.clone().unwrap_or_else(|| "auger/0.1".into()),
            headers: http.headers.clone(),
            basic: None,
            token: None,
            random_ua: false,
            follow_redirects: true,
            http2: http.http2,
            keepalive: !http.no_keepalive,
            worker: 0,
        }
    }

    pub fn worker(mut self, w: usize) -> Self {
        self.worker = w;
        self
    }

    pub fn with_basic(mut self, basic: Option<String>) -> Self {
        self.basic = basic;
        self
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    pub fn with_random_ua(mut self, on: bool) -> Self {
        self.random_ua = on;
        self
    }

    pub fn without_redirects(mut self) -> Self {
        self.follow_redirects = false;
        self
    }

    pub fn build(&self) -> anyhow::Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(self.timeout))
            .user_agent(self.ua());
        if self.insecure {
            builder = builder.danger_accept_invalid_certs(true);
        }
        if let Some(proxy) = &self.proxy {
            let parsed = reqwest::Proxy::all(proxy)
                .with_context(|| format!("invalid proxy '{}'", proxy))?;
            builder = builder.proxy(parsed);
        }
        if !self.follow_redirects {
            builder = builder.redirect(reqwest::redirect::Policy::none());
        }
        if !self.keepalive {
            builder = builder.pool_max_idle_per_host(0);
        }
        if self.http2 {
            builder = builder.http2_prior_knowledge();
        }
        let mut headers = parse_headers(&self.headers)?;
        if let Some(basic) = &self.basic {
            let (user, pass) = basic
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("--basic must be in the form user:pass"))?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", user, pass));
            headers.insert(reqwest::header::AUTHORIZATION, format!("Basic {}", encoded).parse()?);
        }
        if let Some(token) = &self.token {
            headers.insert(reqwest::header::AUTHORIZATION, format!("Bearer {}", token).parse()?);
        }
        builder = builder.default_headers(headers);
        Ok(builder.build()?)
    }

    fn ua(&self) -> String {
        if self.random_ua {
            UA_POOL[(self.worker + 1) % UA_POOL.len()].to_string()
        } else {
            self.user_agent.clone()
        }
    }
}

pub fn parse_headers(items: &[String]) -> anyhow::Result<reqwest::header::HeaderMap> {
    let mut map = reqwest::header::HeaderMap::new();
    for item in items {
        let (name, value) = item
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("header '{}' must be in the form Name: value", item))?;
        map.insert(
            name.trim().parse::<reqwest::header::HeaderName>()?,
            value.trim().parse::<reqwest::header::HeaderValue>()?,
        );
    }
    Ok(map)
}
