//! Where the agent's web tools may go: public http(s) servers only. A web
//! page can talk a model into asking for any address, so the user's own
//! computer, home network and cloud metadata services are out of reach.
//! Checked on the resolved IP address (not just the name), on every redirect,
//! and for every request the browser makes (see `proxy`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::{Host, Url};

/// What may be reached. Tests add their local servers; the app never does.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub(crate) also: Vec<SocketAddr>,
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b)) // carrier NAT, also used by VPNs
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19)))
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    let embedded = |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    // NAT64 and 6to4 carry an IPv4 address inside.
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        return public_v4(embedded(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return public_v4(embedded(s[1], s[2]));
    }
    // Only global unicast (2000::/3), without Teredo and documentation space.
    (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && (s[1] == 0 || s[1] == 0xdb8))
}

pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => public_v4(v),
        IpAddr::V6(v) => public_v6(v),
    }
}

/// A host name that only means something inside a network.
fn local_name(name: &str) -> bool {
    let n = name.trim_end_matches('.').to_ascii_lowercase();
    !n.contains('.') || [".localhost", ".local", ".internal", ".lan", ".home", ".home.arpa", ".intranet"].iter().any(|s| n.ends_with(s))
}

const REFUSED: &str = "That address is on this computer or a private network. Web tools only reach public websites (use the project preview tool for local servers).";

impl Policy {
    /// The addresses of `host`, if every one of them may be reached.
    pub async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let literal: Option<IpAddr> = bare.parse().ok();
        let addrs: Vec<SocketAddr> = match literal {
            Some(ip) => vec![SocketAddr::new(ip, port)],
            None => tokio::net::lookup_host((bare, port)).await.map_err(|_| format!("Could not find the website {bare}"))?.collect(),
        };
        if addrs.iter().any(|a| self.also.contains(a)) {
            return Ok(addrs);
        }
        if (literal.is_none() && local_name(bare)) || addrs.is_empty() || !addrs.iter().all(|a| is_public(a.ip())) {
            return Err(REFUSED.into());
        }
        Ok(addrs)
    }

    /// Parse and check a URL the model asked for. "example.com" becomes https.
    pub async fn check(&self, raw: &str) -> Result<Url, String> {
        let raw = raw.trim();
        if raw.is_empty() || raw.len() > 4000 {
            return Err("Give a web address".into());
        }
        let with_scheme = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
        let url = Url::parse(&with_scheme).map_err(|_| format!("Not a web address: {raw}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("Only http and https pages can be opened".into());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("Addresses with a user name or password in them are not opened".into());
        }
        let host = match url.host() {
            Some(Host::Domain(d)) => d.to_string(),
            Some(Host::Ipv4(ip)) => ip.to_string(),
            Some(Host::Ipv6(ip)) => ip.to_string(),
            None => return Err("That address has no host".into()),
        };
        self.resolve(&host, url.port_or_known_default().unwrap_or(443)).await?;
        Ok(url)
    }
}

/// reqwest resolves names through the policy, so the address that was
/// checked is the address that is connected to.
pub(crate) struct Resolver(pub Policy);

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let policy = self.0.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = policy.resolve(&host, 0).await?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}
