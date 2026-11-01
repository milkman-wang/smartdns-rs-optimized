//! The addresses this server answers on, so that no upstream is this server.
//!
//! A query sent to an upstream that is this server comes back as a new query,
//! which goes upstream again: the lookups never end, and their pending queries
//! pile up until memory runs out. The usual way in is a configuration without
//! `server` on OpenWrt: the fallback, /etc/resolv.conf, names 127.0.0.1 and
//! ::1, where this server listens on port 53.

use std::net::{IpAddr, SocketAddr};
use std::sync::RwLock;

use crate::config::{BindAddrConfig, IBindConfig};
use crate::dns_url::{DnsUrl, ProtocolConfig};
use crate::log::warn;

/// How a listener or an upstream carries DNS messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// UDP and TCP, which share a port.
    Plain,
    Tls,
    Https,
    Quic,
    H3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listener {
    pub transport: Transport,
    pub addr: SocketAddr,
}

/// The running server's listeners, for the upstreams of `server system` and
/// `server dhcp`, found only when they connect.
static LISTENERS: RwLock<Vec<Listener>> = RwLock::new(Vec::new());

/// The listeners of the enabled binds, at the addresses the server binds.
pub fn from_binds(binds: &[BindAddrConfig]) -> Vec<Listener> {
    binds
        .iter()
        .filter(|bind| bind.enabled())
        .map(|bind| Listener {
            transport: match bind {
                BindAddrConfig::Udp(_) | BindAddrConfig::Tcp(_) => Transport::Plain,
                BindAddrConfig::Tls(_) => Transport::Tls,
                BindAddrConfig::Http(_) | BindAddrConfig::Https(_) => Transport::Https,
                BindAddrConfig::Quic(_) => Transport::Quic,
                BindAddrConfig::H3(_) => Transport::H3,
            },
            addr: bind.sock_addr(),
        })
        .collect()
}

pub fn set(listeners: Vec<Listener>) {
    *LISTENERS.write().unwrap_or_else(|err| err.into_inner()) = listeners;
}

pub fn get() -> Vec<Listener> {
    LISTENERS
        .read()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
}

/// The addresses of the local interfaces. Loopback addresses count as local
/// without them.
pub fn local_ips() -> Vec<IpAddr> {
    crate::infra::net::list_afinet_netifas()
        .map(|ifas| ifas.into_iter().map(|(_, ip)| ip).collect())
        .unwrap_or_default()
}

fn transport(url: &DnsUrl) -> Transport {
    match url.proto() {
        ProtocolConfig::Udp
        | ProtocolConfig::Tcp
        | ProtocolConfig::System
        | ProtocolConfig::Dhcp { .. } => Transport::Plain,
        #[cfg(feature = "dns-over-tls")]
        ProtocolConfig::Tls => Transport::Tls,
        #[cfg(feature = "dns-over-https")]
        ProtocolConfig::Https { .. } => Transport::Https,
        #[cfg(feature = "dns-over-quic")]
        ProtocolConfig::Quic => Transport::Quic,
        #[cfg(feature = "dns-over-h3")]
        ProtocolConfig::H3 { .. } => Transport::H3,
    }
}

/// Whether the upstream `url`, if it has an address, is one of `listeners`.
pub fn is_own(listeners: &[Listener], url: &DnsUrl, local_ips: &[IpAddr]) -> bool {
    let Some(ip) = url.ip() else {
        return false;
    };
    let (ip, port, transport) = (ip.to_canonical(), url.port(), transport(url));
    listeners.iter().any(|listener| {
        let bound = listener.addr.ip().to_canonical();
        listener.transport == transport
            && listener.addr.port() == port
            && if bound.is_unspecified() {
                // An IPv6 wildcard takes IPv4 too: listeners are dual-stack.
                (bound.is_ipv6() || ip.is_ipv4())
                    && (ip.is_loopback() || ip.is_unspecified() || local_ips.contains(&ip))
            } else {
                bound == ip
            }
    })
}

/// `urls` without those that are one of `listeners`, and how many were left out.
pub fn without_own(
    listeners: &[Listener],
    urls: impl IntoIterator<Item = DnsUrl>,
) -> (Vec<DnsUrl>, usize) {
    let local_ips = if listeners.is_empty() {
        vec![]
    } else {
        local_ips()
    };
    let mut skipped = 0;
    let urls = urls
        .into_iter()
        .filter(|url| {
            let own = is_own(listeners, url, &local_ips);
            if own {
                warn!("upstream {url} is this server itself; skipped to avoid a query loop");
                skipped += 1;
            }
            !own
        })
        .collect();
    (urls, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener(transport: Transport, addr: &str) -> Listener {
        Listener {
            transport,
            addr: addr.parse().unwrap(),
        }
    }

    fn own(listeners: &[Listener], url: &str) -> bool {
        let local = ["192.168.1.1".parse().unwrap()];
        is_own(listeners, &url.parse().unwrap(), &local)
    }

    #[test]
    fn test_wildcard_listeners_take_local_addresses() {
        let v6 = [listener(Transport::Plain, "[::]:53")];
        assert!(own(&v6, "udp://127.0.0.1"));
        assert!(own(&v6, "tcp://[::1]:53"));
        assert!(own(&v6, "udp://192.168.1.1:53"));
        assert!(!own(&v6, "udp://192.168.1.2:53"));
        assert!(!own(&v6, "udp://127.0.0.1:5353"));

        // A plain IPv4 wildcard is not reached over IPv6.
        let v4 = [listener(Transport::Plain, "0.0.0.0:53")];
        assert!(own(&v4, "udp://127.0.0.1"));
        assert!(!own(&v4, "udp://[::1]:53"));
    }

    #[test]
    fn test_specific_listeners_take_their_own_address() {
        let lan = [listener(Transport::Plain, "192.168.1.1:15360")];
        assert!(own(&lan, "udp://192.168.1.1:15360"));
        assert!(!own(&lan, "udp://127.0.0.1:15360"));
    }

    #[cfg(feature = "dns-over-tls")]
    #[test]
    fn test_only_the_same_transport_counts() {
        let listeners = [listener(Transport::Plain, "[::]:53")];
        assert!(!own(&listeners, "tls://127.0.0.1:53"));
        let listeners = [listener(Transport::Tls, "[::]:853")];
        assert!(own(&listeners, "tls://127.0.0.1"));
        assert!(!own(&listeners, "udp://127.0.0.1:853"));
    }
}
