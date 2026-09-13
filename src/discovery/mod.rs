//! Email settings discovery for the account configuration wizard
//! (ADR 0003 §3.3).
//!
//! A thin adapter isolates the volatile 0.x `io-pim-discovery` API
//! behind the [`EmailConfigDiscoverer`] trait object: the rest of
//! Tmail (wizard state, reducer, UI) only ever sees the stable
//! [`DiscoveredService`] shape. The real [`PimDiscoverer`] runs the
//! crate's blocking compose client on a worker thread, bounded by a
//! deadline; tests and smoke runs inject [`FakeDiscoverer`] (or any
//! other trait impl) at the dependency-injection point in `main.rs`.
//!
//! Discovery results are filtered to IMAP incoming and SMTP mail
//! submission endpoints. POP results are discarded — himalaya 2.x has
//! no POP3 backend — and JMAP results are ignored in v1 (future work).

mod alias;

pub use alias::{ALIAS_ROLES, derive_aliases};

use std::collections::BTreeSet;
use std::time::Duration;

use io_pim_discovery::compose::client::DiscoveryComposeClientStd;
use io_pim_discovery::compose::config::{
    DiscoveryConfigSource, DiscoveryEndpoint, DiscoverySecurity, DiscoveryService,
    DiscoveryServiceConfig,
};
use io_pim_discovery::compose::providers::DiscoveryKnownProvider;
use io_pim_discovery::shared::dns::system_resolver;
use pimalaya_stream::tls::Tls;
use url::Url;

/// How long the composed discovery may run before mechanisms that are
/// still probing are abandoned (ADR 0003 §3.2 W2: deadline 15 s).
pub const DISCOVERY_DEADLINE: Duration = Duration::from_secs(15);

/// Fallback DNS-over-TCP resolver, mirroring the `pim-discovery` CLI
/// default. Used only when [`system_resolver`] cannot determine the
/// system's nameserver. Parsed once at first use (const-verified by a
/// test); the panic then can never fire during a request.
static DEFAULT_DNS_RESOLVER: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    Url::parse("tcp://1.1.1.1:53").expect("default DNS resolver URL is valid")
});

/// Extra slack over [`DISCOVERY_DEADLINE`] for the `spawn_blocking`
/// hop itself before the async wrapper gives up on the join handle.
const BLOCKING_JOIN_SLACK: Duration = Duration::from_secs(1);

/// Transport security of a discovered server endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    /// Implicit TLS from the first byte (`imaps://`, `smtps://`).
    Tls,
    /// Opportunistic TLS via the STARTTLS command (`imap://`,
    /// `smtp://` plus `starttls = true` in the himalaya config).
    StartTls,
    /// Unencrypted connection (`imap://`, `smtp://`).
    Plain,
}

/// One discovered server endpoint, already mapped to the URL shape
/// himalaya expects in `imap.server` / `smtp.server`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEndpoint {
    /// e.g. `imaps://imap.gmail.com:993`.
    pub url: String,
    /// How TLS is negotiated on this endpoint.
    pub security: Security,
}

impl ServerEndpoint {
    /// The `imap.starttls` / `smtp.starttls` flag value for the
    /// himalaya account block: `true` only for STARTTLS endpoints.
    pub fn starttls(&self) -> bool {
        self.security == Security::StartTls
    }
}

/// A provider covered by fixed rules (display + wizard presets such as
/// the Gmail mailbox-alias table and the app-password hint).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Google (Gmail, Google Workspace).
    Gmail,
    /// Microsoft (Outlook.com, Microsoft 365).
    Outlook,
}

impl Provider {
    /// Human-readable provider name for UI labels.
    pub fn name(self) -> &'static str {
        match self {
            Provider::Gmail => "Gmail",
            Provider::Outlook => "Outlook",
        }
    }

    /// Infers the provider from a server hostname (ADR 0003 §3.5:
    /// the Gmail alias preset also applies when the IMAP host is
    /// `imap.gmail.com` even without a discovery provider tag).
    pub fn from_host(host: &str) -> Option<Self> {
        let host = host.to_ascii_lowercase();
        if host.contains("gmail.com") || host.contains("googlemail.com") {
            Some(Provider::Gmail)
        } else if ["outlook.", "hotmail.", "live.", "office365."]
            .iter()
            .any(|marker| host.contains(marker))
        {
            Some(Provider::Outlook)
        } else {
            None
        }
    }
}

/// Where a discovered setting came from (UI label + ranking).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    /// A fixed provider rule (domain or MX match).
    Provider(Provider),
    /// Mozilla Thunderbird autoconfig (ISP URLs, mailconf redirect,
    /// Thunderbird ISPDB).
    Autoconfig,
    /// PACC discovery (draft-ietf-mailmaint-pacc).
    Pacc,
    /// RFC 6186 DNS SRV records.
    Rfc6186,
    /// Typed by the user in the manual-override form.
    Manual,
}

impl ConfigSource {
    /// Row label shown on the discovery screen (ADR 0003 §3.2 W2).
    pub fn label(self) -> String {
        match self {
            ConfigSource::Provider(p) => format!("known provider: {}", p.name()),
            ConfigSource::Autoconfig => "Thunderbird autoconfig".to_string(),
            ConfigSource::Pacc => "PACC".to_string(),
            ConfigSource::Rfc6186 => "DNS SRV (RFC 6186)".to_string(),
            ConfigSource::Manual => "manual".to_string(),
        }
    }

    /// Ranking priority among mechanisms: PACC and autoconfig beat
    /// RFC 6186 SRV (ADR 0003 §3.3). Manual candidates never take
    /// part in ranking.
    fn mechanism_rank(self) -> u8 {
        match self {
            ConfigSource::Provider(_) | ConfigSource::Manual => 0,
            ConfigSource::Pacc | ConfigSource::Autoconfig => 1,
            ConfigSource::Rfc6186 => 2,
        }
    }
}

/// One ranked IMAP(+SMTP) candidate pair shown on the discovery
/// screen. Single-sided discoveries keep `smtp: None` and are flagged
/// `smtp: not found`; the user can supply SMTP in the manual-override
/// form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredService {
    /// Mechanism that produced the IMAP endpoint (row label).
    pub source: ConfigSource,
    /// The IMAP endpoint.
    pub imap: ServerEndpoint,
    /// The paired SMTP endpoint, when one was discovered.
    pub smtp: Option<ServerEndpoint>,
    /// Provider tag, when the endpoint host or the discovery source
    /// identifies a known provider.
    pub provider: Option<Provider>,
    /// The login the discovery mechanism advertised, when any.
    pub username: Option<String>,
}

/// Discovers IMAP/SMTP settings for an email address. Implementations
/// must never block the async caller: the real one hops to a worker
/// thread; fakes return canned data.
#[async_trait::async_trait]
pub trait EmailConfigDiscoverer: Send + Sync {
    /// The ranked candidate list, or `Err` with a human-readable reason
    /// when discovery itself broke. `Ok(empty)` strictly means "no
    /// candidate mechanisms answered in time" (the wizard then offers
    /// manual override) — so the wizard can tell the user *why* the
    /// results are empty instead of conflating the two (review s843).
    async fn discover(&self, email: &str) -> Result<Vec<DiscoveredService>, String>;
}

/// The real discoverer, wrapping `io-pim-discovery`'s blocking
/// compose client (ADR 0003 §3.3).
pub struct PimDiscoverer;

#[async_trait::async_trait]
impl EmailConfigDiscoverer for PimDiscoverer {
    async fn discover(&self, email: &str) -> Result<Vec<DiscoveredService>, String> {
        // The compose client blocks and spawns its own mechanism
        // threads, so it must leave the async reactor: run it on the
        // blocking pool. `compose_all_within` already bounds the wait
        // to the deadline (still-running mechanisms are abandoned in
        // the background); the outer timeout only guards the
        // spawn_blocking hop itself.
        let email = email.to_string();
        let handle = tokio::task::spawn_blocking(move || discover_blocking(&email));
        match tokio::time::timeout(DISCOVERY_DEADLINE + BLOCKING_JOIN_SLACK, handle).await {
            Ok(Ok(inner)) => inner,
            Ok(Err(err)) => Err(format!("discovery worker failed to run: {err}")),
            Err(_) => Err(String::from(
                "discovery did not answer in time (17s deadline)",
            )),
        }
    }
}

/// Runs the blocking compose client to completion. Called from the
/// blocking pool only.
fn discover_blocking(email: &str) -> Result<Vec<DiscoveredService>, String> {
    let dns = system_resolver().unwrap_or_else(|| DEFAULT_DNS_RESOLVER.clone());
    let client = DiscoveryComposeClientStd::new(dns, Tls::default());
    // Only the services himalaya can drive: IMAP incoming and SMTP
    // submission. Mechanisms irrelevant to the requested services are
    // never started, so no POP discovery work happens at all.
    let services = BTreeSet::from([DiscoveryService::Imap, DiscoveryService::Smtp]);
    match client.compose_all_within(email, services, DISCOVERY_DEADLINE) {
        Ok(configs) => Ok(compose_candidates(configs)),
        // Only an invalid email fails the whole compose; the wizard
        // validates the address first, so a compose error is a
        // discoverer-level error surfaced as `Err`, never hidden as an
        // empty result (review s843).
        Err(err) => {
            tracing::warn!(error = %err, "discovery compose failed");
            Err(format!("discovery failed: {err}"))
        }
    }
}

/// Reduces raw discovery configs into ranked, paired candidates
/// (ADR 0003 §3.3: pairing, URL mapping, ranking).
pub(crate) fn compose_candidates(configs: Vec<DiscoveryServiceConfig>) -> Vec<DiscoveredService> {
    // Keep only IMAP and SMTP; POP3/JMAP/DAV/Sieve results are
    // discarded (no himalaya backend for them in v1).
    let kept: Vec<&DiscoveryServiceConfig> = configs
        .iter()
        .filter(|config| {
            matches!(
                config.service,
                DiscoveryService::Imap | DiscoveryService::Smtp
            )
        })
        .collect();

    // Group by source in first-appearance (collector) order so ties
    // keep the collector's order.
    let mut groups: Vec<(DiscoveryConfigSource, Vec<&DiscoveryServiceConfig>)> = Vec::new();
    for config in &kept {
        match groups
            .iter_mut()
            .find(|(source, _)| *source == config.source)
        {
            Some((_, group)) => group.push(*config),
            None => groups.push((config.source, vec![*config])),
        }
    }

    let mut candidates = Vec::new();
    for (source, group) in &groups {
        // First IMAP endpoint of the group; the SMTP endpoint prefers
        // the same source and falls back to any SMTP endpoint.
        let imap = group
            .iter()
            .find(|config| config.service == DiscoveryService::Imap);
        let Some(imap) = imap else { continue };
        let smtp = group
            .iter()
            .find(|config| config.service == DiscoveryService::Smtp)
            .copied()
            .or_else(|| {
                kept.iter().copied().find(|config| {
                    config.service == DiscoveryService::Smtp && config.source != *source
                })
            });
        candidates.push(build_candidate(*source, imap, smtp));
    }

    rank_candidates(candidates)
}

/// Maps one IMAP(+SMTP) config pair into the stable candidate shape.
fn build_candidate(
    source: DiscoveryConfigSource,
    imap: &DiscoveryServiceConfig,
    smtp: Option<&DiscoveryServiceConfig>,
) -> DiscoveredService {
    let provider = provider_of(source, imap);
    DiscoveredService {
        source: map_source(source),
        imap: map_endpoint(imap),
        smtp: smtp.map(map_endpoint),
        provider,
        username: imap.username.clone(),
    }
}

/// Maps a mechanism source to the adapter's stable source enum.
fn map_source(source: DiscoveryConfigSource) -> ConfigSource {
    match source {
        DiscoveryConfigSource::Provider(DiscoveryKnownProvider::Google) => {
            ConfigSource::Provider(Provider::Gmail)
        }
        DiscoveryConfigSource::Provider(DiscoveryKnownProvider::Microsoft) => {
            ConfigSource::Provider(Provider::Outlook)
        }
        DiscoveryConfigSource::Pacc => ConfigSource::Pacc,
        DiscoveryConfigSource::Srv => ConfigSource::Rfc6186,
        // IspMain, IspFallback, Mailconf and Ispdb are all Mozilla
        // autoconfig flavors; Dav and Jmap are filtered out upstream.
        _ => ConfigSource::Autoconfig,
    }
}

/// Derives the provider tag from the source first, then the IMAP
/// endpoint host.
fn provider_of(source: DiscoveryConfigSource, imap: &DiscoveryServiceConfig) -> Option<Provider> {
    match source {
        DiscoveryConfigSource::Provider(DiscoveryKnownProvider::Google) => Some(Provider::Gmail),
        DiscoveryConfigSource::Provider(DiscoveryKnownProvider::Microsoft) => {
            Some(Provider::Outlook)
        }
        _ => match &imap.endpoint {
            DiscoveryEndpoint::Tcp { host, .. } => Provider::from_host(host),
            _ => None,
        },
    }
}

/// Maps a discovered endpoint to the himalaya URL shape (ADR 0003
/// §3.3): implicit TLS → `imaps://`/`smtps://`, STARTTLS and
/// plaintext → `imap://`/`smtp://` with the `starttls` flag carried
/// separately on [`ServerEndpoint`].
fn map_endpoint(config: &DiscoveryServiceConfig) -> ServerEndpoint {
    let DiscoveryEndpoint::Tcp {
        host,
        port,
        security,
    } = &config.endpoint
    else {
        // HTTP endpoints (JMAP, DAV) never reach this adapter.
        return ServerEndpoint {
            url: String::new(),
            security: Security::Plain,
        };
    };
    let (scheme, security) = match (config.service, security) {
        (DiscoveryService::Imap, DiscoverySecurity::Tls) => ("imaps", Security::Tls),
        (DiscoveryService::Smtp, DiscoverySecurity::Tls) => ("smtps", Security::Tls),
        (DiscoveryService::Imap, _) => ("imap", map_security(*security)),
        (DiscoveryService::Smtp, _) => ("smtp", map_security(*security)),
        _ => ("imap", map_security(*security)),
    };
    ServerEndpoint {
        url: format!("{scheme}://{host}:{port}"),
        security,
    }
}

fn map_security(security: DiscoverySecurity) -> Security {
    match security {
        DiscoverySecurity::Tls => Security::Tls,
        DiscoverySecurity::Starttls => Security::StartTls,
        DiscoverySecurity::Plain => Security::Plain,
    }
}

/// Orders candidates: known-provider rules first, then TLS endpoints
/// over STARTTLS over plaintext, then mechanism priority. Stable, so
/// ties keep the collector's order.
fn rank_candidates(mut candidates: Vec<DiscoveredService>) -> Vec<DiscoveredService> {
    candidates.sort_by_key(|service| {
        (
            u8::from(!matches!(service.source, ConfigSource::Provider(_))),
            match service.imap.security {
                Security::Tls => 0u8,
                Security::StartTls => 1,
                Security::Plain => 2,
            },
            service.source.mechanism_rank(),
        )
    });
    candidates
}

/// Canned fake for tests and smoke runs (`TMAIL_FAKE_DISCOVERY=1` at
/// the dependency-injection point): never touches the network. shaped
/// after the real Google fixed rules so the smoke run exercises the
/// Gmail alias preset and the app-password hint.
pub struct FakeDiscoverer;

#[async_trait::async_trait]
impl EmailConfigDiscoverer for FakeDiscoverer {
    async fn discover(&self, _email: &str) -> Result<Vec<DiscoveredService>, String> {
        Ok(vec![DiscoveredService {
            source: ConfigSource::Provider(Provider::Gmail),
            imap: ServerEndpoint {
                url: "imaps://imap.gmail.com:993".to_string(),
                security: Security::Tls,
            },
            smtp: Some(ServerEndpoint {
                url: "smtps://smtp.gmail.com:465".to_string(),
                security: Security::Tls,
            }),
            provider: Some(Provider::Gmail),
            username: None,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dns_resolver_url_is_valid() {
        // Fires the LazyLock initializer at test time: an invalid const
        // URL is caught here, not on a user's first discovery run.
        assert_eq!(DEFAULT_DNS_RESOLVER.as_str(), "tcp://1.1.1.1:53");
    }

    fn tcp(
        service: DiscoveryService,
        host: &str,
        port: u16,
        security: DiscoverySecurity,
    ) -> DiscoveryEndpoint {
        let _ = service;
        DiscoveryEndpoint::Tcp {
            host: host.to_string(),
            port,
            security,
        }
    }

    fn config(
        service: DiscoveryService,
        endpoint: DiscoveryEndpoint,
        source: DiscoveryConfigSource,
    ) -> DiscoveryServiceConfig {
        DiscoveryServiceConfig {
            service,
            endpoint,
            username: None,
            auth: Vec::new(),
            source,
        }
    }

    #[test]
    fn imap_url_mapping_matches_himalaya_shape() {
        let tls = config(
            DiscoveryService::Imap,
            tcp(
                DiscoveryService::Imap,
                "imap.gmail.com",
                993,
                DiscoverySecurity::Tls,
            ),
            DiscoveryConfigSource::Srv,
        );
        assert_eq!(
            map_endpoint(&tls).url,
            "imaps://imap.gmail.com:993",
            "implicit TLS must map to imaps://"
        );
        assert_eq!(map_endpoint(&tls).security, Security::Tls);

        let starttls = config(
            DiscoveryService::Imap,
            tcp(
                DiscoveryService::Imap,
                "imap.example.com",
                143,
                DiscoverySecurity::Starttls,
            ),
            DiscoveryConfigSource::IspMain,
        );
        let mapped = map_endpoint(&starttls);
        assert_eq!(mapped.url, "imap://imap.example.com:143");
        assert_eq!(mapped.security, Security::StartTls);
        assert!(mapped.starttls(), "STARTTLS endpoints must flag starttls");

        let plain = config(
            DiscoveryService::Smtp,
            tcp(
                DiscoveryService::Smtp,
                "smtp.example.com",
                25,
                DiscoverySecurity::Plain,
            ),
            DiscoveryConfigSource::Srv,
        );
        let mapped = map_endpoint(&plain);
        assert_eq!(mapped.url, "smtp://smtp.example.com:25");
        assert!(
            !mapped.starttls(),
            "plaintext endpoints must not flag starttls"
        );
    }

    #[test]
    fn pop_and_jmap_results_are_discarded() {
        let configs = vec![
            config(
                DiscoveryService::Pop3,
                tcp(
                    DiscoveryService::Pop3,
                    "pop.example.com",
                    995,
                    DiscoverySecurity::Tls,
                ),
                DiscoveryConfigSource::Ispdb,
            ),
            config(
                DiscoveryService::Jmap,
                DiscoveryEndpoint::Http("https://jmap.example.com/session".to_string()),
                DiscoveryConfigSource::Jmap,
            ),
            config(
                DiscoveryService::Imap,
                tcp(
                    DiscoveryService::Imap,
                    "imap.example.com",
                    993,
                    DiscoverySecurity::Tls,
                ),
                DiscoveryConfigSource::Ispdb,
            ),
        ];

        let candidates = compose_candidates(configs);

        assert_eq!(
            candidates.len(),
            1,
            "only the IMAP config yields a candidate"
        );
        assert!(
            candidates[0].smtp.is_none(),
            "single-sided candidate has no SMTP"
        );
        assert_eq!(candidates[0].imap.url, "imaps://imap.example.com:993");
    }

    #[test]
    fn same_source_smtp_is_paired_before_global_fallback() {
        let configs = vec![
            // SRV: IMAP only.
            config(
                DiscoveryService::Imap,
                tcp(
                    DiscoveryService::Imap,
                    "imap.example.com",
                    993,
                    DiscoverySecurity::Tls,
                ),
                DiscoveryConfigSource::Srv,
            ),
            // Autoconfig: SMTP only.
            config(
                DiscoveryService::Smtp,
                tcp(
                    DiscoveryService::Smtp,
                    "smtp.example.com",
                    587,
                    DiscoverySecurity::Starttls,
                ),
                DiscoveryConfigSource::IspMain,
            ),
        ];

        let candidates = compose_candidates(configs);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].source, ConfigSource::Rfc6186);
        assert_eq!(
            candidates[0]
                .smtp
                .as_ref()
                .expect("global SMTP fallback pairs the row")
                .url,
            "smtp://smtp.example.com:587"
        );
    }

    #[test]
    fn ranking_prefers_provider_then_tls_then_mechanism() {
        let configs = vec![
            // Would win on mechanism priority, but STARTTLS loses to TLS.
            config(
                DiscoveryService::Imap,
                tcp(
                    DiscoveryService::Imap,
                    "imap.example.com",
                    143,
                    DiscoverySecurity::Starttls,
                ),
                DiscoveryConfigSource::Pacc,
            ),
            config(
                DiscoveryService::Smtp,
                tcp(
                    DiscoveryService::Smtp,
                    "smtp.example.com",
                    587,
                    DiscoverySecurity::Starttls,
                ),
                DiscoveryConfigSource::Pacc,
            ),
            // SRV TLS beats autoconfig STARTTLS (security ranks first).
            config(
                DiscoveryService::Imap,
                tcp(
                    DiscoveryService::Imap,
                    "imap.example.com",
                    993,
                    DiscoverySecurity::Tls,
                ),
                DiscoveryConfigSource::Srv,
            ),
            // Known-provider rule beats everything.
            config(
                DiscoveryService::Imap,
                tcp(
                    DiscoveryService::Imap,
                    "imap.gmail.com",
                    993,
                    DiscoverySecurity::Tls,
                ),
                DiscoveryConfigSource::Provider(DiscoveryKnownProvider::Google),
            ),
            config(
                DiscoveryService::Smtp,
                tcp(
                    DiscoveryService::Smtp,
                    "smtp.gmail.com",
                    465,
                    DiscoverySecurity::Tls,
                ),
                DiscoveryConfigSource::Provider(DiscoveryKnownProvider::Google),
            ),
        ];

        let candidates = compose_candidates(configs);

        let sources: Vec<ConfigSource> = candidates.iter().map(|c| c.source).collect();
        assert_eq!(
            sources,
            vec![
                ConfigSource::Provider(Provider::Gmail),
                ConfigSource::Rfc6186,
                ConfigSource::Pacc,
            ],
            "provider rules first, then TLS over STARTTLS, then mechanism priority"
        );
    }

    #[test]
    fn provider_tag_falls_back_to_imap_host() {
        let configs = vec![config(
            DiscoveryService::Imap,
            tcp(
                DiscoveryService::Imap,
                "imap.gmail.com",
                993,
                DiscoverySecurity::Tls,
            ),
            DiscoveryConfigSource::Srv,
        )];

        let candidates = compose_candidates(configs);

        assert_eq!(candidates[0].provider, Some(Provider::Gmail));
    }

    #[test]
    fn source_labels_match_the_adr_copy() {
        assert_eq!(
            ConfigSource::Provider(Provider::Gmail).label(),
            "known provider: Gmail"
        );
        assert_eq!(ConfigSource::Autoconfig.label(), "Thunderbird autoconfig");
        assert_eq!(ConfigSource::Pacc.label(), "PACC");
        assert_eq!(ConfigSource::Rfc6186.label(), "DNS SRV (RFC 6186)");
        assert_eq!(ConfigSource::Manual.label(), "manual");
    }

    /// Deserializes a recorded mechanism output (serde camelCase) and
    /// runs the full compose pipeline over it.
    fn fixture_configs(name: &str) -> Vec<DiscoveryServiceConfig> {
        let raw = std::fs::read_to_string(format!("fixtures/discovery/{name}"))
            .unwrap_or_else(|err| panic!("fixture {name} must be readable: {err}"));
        serde_json::from_str(&raw).expect("fixture must deserialize into discovery configs")
    }

    #[test]
    fn gmail_provider_fixture_composes_one_ranked_pair() {
        let candidates = compose_candidates(fixture_configs("gmail_provider.json"));

        assert_eq!(
            candidates.len(),
            1,
            "Google's fixed rules yield one candidate"
        );
        let gmail = &candidates[0];
        assert_eq!(gmail.source, ConfigSource::Provider(Provider::Gmail));
        assert_eq!(gmail.provider, Some(Provider::Gmail));
        assert_eq!(gmail.imap.url, "imaps://imap.gmail.com:993");
        assert_eq!(
            gmail.smtp.as_ref().expect("Google publishes SMTP too").url,
            "smtps://smtp.gmail.com:465"
        );
        assert_eq!(gmail.username.as_deref(), Some("user@gmail.com"));
    }

    #[test]
    fn autoconfig_and_srv_fixture_ranks_tls_over_starttls() {
        let candidates = compose_candidates(fixture_configs("autoconfig_and_srv.json"));

        let sources: Vec<ConfigSource> = candidates.iter().map(|c| c.source).collect();
        assert_eq!(
            sources,
            vec![ConfigSource::Rfc6186, ConfigSource::Autoconfig],
            "the SRV TLS pair outranks the autoconfig STARTTLS pair"
        );
        assert_eq!(candidates[0].imap.url, "imaps://imap.example.com:993");
        // The SRV group has no SMTP of its own: the autoconfig SMTP
        // pairs in as the global fallback.
        assert_eq!(
            candidates[0].smtp.as_ref().expect("fallback SMTP").url,
            "smtp://smtp.example.com:587"
        );
        assert_eq!(candidates[1].imap.url, "imap://imap.example.com:143");
        assert!(candidates[1].imap.starttls());
        assert_eq!(
            candidates[1].smtp.as_ref().expect("same-source SMTP").url,
            "smtp://smtp.example.com:587"
        );
    }
}
