//! Discovery of Designated Resolvers (RFC 9461 / RFC 9462).
use crate::libdns::proto::rr::rdata::{
    A, AAAA,
    svcb::{Alpn, IpHint, SVCB, SvcParamKey, SvcParamValue, Unknown},
};
use crate::{
    config::{BindAddrConfig, IBindConfig},
    dns::*,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;

static RESERVED: LazyLock<Name> = LazyLock::new(|| "resolver.arpa.".parse().unwrap());
static DISCOVERY: LazyLock<Name> = LazyLock::new(|| "_dns.resolver.arpa.".parse().unwrap());

fn local_ip(ctx: &DnsContext, req: &DnsRequest) -> Option<IpAddr> {
    if let Some(address) = ctx.server_opts.local_addr
        && !address.ip().is_unspecified()
    {
        return Some(address.ip());
    }
    let peer = req.src();
    if peer.ip().is_unspecified() {
        return None;
    }
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(peer),
        socket2::Type::DGRAM,
        None,
    )
    .ok()?;
    socket
        .connect(&SocketAddr::new(peer.ip(), 53).into())
        .ok()?;
    Some(socket.local_addr().ok()?.as_socket()?.ip())
}

pub fn lookup(ctx: &DnsContext, req: &DnsRequest) -> Option<DnsResponse> {
    let query = req.query().original();
    let in_reserved_zone = RESERVED.zone_of(query.name());
    let cfg = ctx.cfg();
    if !in_reserved_zone
        && !cfg
            .binds()
            .iter()
            .any(|bind| bind.enabled() && bind.server_opts().ddr == Some(true))
    {
        return None;
    }
    let mut result = DnsResponse::new_with_max_ttl(query.clone(), []);
    let ttl = cfg.local_ttl() as u32;
    let mut priority = 1;
    let mut target_match = false;
    for bind in cfg
        .binds()
        .iter()
        .filter(|bind| bind.enabled() && bind.server_opts().ddr == Some(true))
    {
        let (alpn, https, ssl) = match bind {
            #[cfg(feature = "dns-over-tls")]
            BindAddrConfig::Tls(bind) => ("dot", false, &bind.ssl_config),
            #[cfg(feature = "dns-over-https")]
            BindAddrConfig::Https(bind) => ("h2", true, &bind.ssl_config),
            #[cfg(feature = "dns-over-quic")]
            BindAddrConfig::Quic(bind) => ("doq", false, &bind.ssl_config),
            #[cfg(feature = "dns-over-h3")]
            BindAddrConfig::H3(bind) => ("h3", true, &bind.ssl_config),
            _ => continue,
        };
        let mut target = ssl
            .server_name
            .as_deref()
            .and_then(|name| name.parse::<Name>().ok())
            .unwrap_or_else(|| cfg.server_name());
        target.set_fqdn(true);
        if target.is_root() || RESERVED.zone_of(&target) {
            continue;
        }
        let named_discovery = target.prepend_label("_dns").ok();
        let requested_svcb = query.query_type() == RecordType::SVCB
            && (query.name() == &*DISCOVERY || named_discovery.as_ref() == Some(query.name()));
        let requested_address = query.name() == &target && query.query_type().is_ip_addr();
        if !requested_svcb && !requested_address {
            continue;
        }
        target_match = true;
        let address = if bind.sock_addr().ip().is_unspecified() {
            local_ip(ctx, req)
        } else {
            Some(bind.sock_addr().ip())
        };
        if requested_address {
            if let Some(ip) = address
                && ((ip.is_ipv4() && query.query_type() == RecordType::A)
                    || (ip.is_ipv6() && query.query_type() == RecordType::AAAA))
            {
                let record = Record::from_rdata(target.clone(), ttl, ip.into());
                if !result.answers().contains(&record) {
                    result.add_answer(record);
                }
            }
            continue;
        }
        let mut params = vec![
            (
                SvcParamKey::Alpn,
                SvcParamValue::Alpn(Alpn(vec![alpn.into()])),
            ),
            (SvcParamKey::Port, SvcParamValue::Port(bind.port())),
        ];
        if let Some(ip) = address {
            params.push(match ip {
                IpAddr::V4(ip) => (
                    SvcParamKey::Ipv4Hint,
                    SvcParamValue::Ipv4Hint(IpHint(vec![A(ip)])),
                ),
                IpAddr::V6(ip) => (
                    SvcParamKey::Ipv6Hint,
                    SvcParamValue::Ipv6Hint(IpHint(vec![AAAA(ip)])),
                ),
            });
            result.add_additional(Record::from_rdata(target.clone(), ttl, ip.into()));
        }
        if https {
            params.push((
                SvcParamKey::Unknown(7),
                SvcParamValue::Unknown(Unknown(b"/dns-query{?dns}".to_vec())),
            ));
        }
        result.add_answer(Record::from_rdata(
            query.name().clone(),
            ttl,
            RData::SVCB(SVCB::new(priority, target, params)),
        ));
        priority += 1;
    }
    if !in_reserved_zone && !target_match {
        return None;
    }
    if result.answers().is_empty() {
        result.add_authority(Record::from_rdata(
            query.name().clone(),
            ttl,
            RData::default_soa(),
        ));
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dns_conf::RuntimeConfig, dns_mw::DnsMockMiddleware, dns_mw_zone::DnsZoneMiddleware,
    };
    #[tokio::test]
    async fn test_ddr_advertises_enabled_encrypted_listeners() {
        let cfg = RuntimeConfig::builder()
            .with("server-name resolver.example")
            .with("bind-tls 192.0.2.53:8853 -ddr")
            .with("bind-https 192.0.2.53:8443 -ddr")
            .with("bind-tls 192.0.2.53:9853")
            .build()
            .unwrap();
        let handler = DnsMockMiddleware::mock(DnsZoneMiddleware::new()).build(cfg);
        let response = handler
            .lookup("_dns.resolver.arpa.", RecordType::SVCB)
            .await
            .unwrap();
        assert_eq!(response.answers().len(), 2);
        let RData::SVCB(dot) = response.answers()[0].data() else {
            panic!("missing SVCB")
        };
        assert_eq!(dot.target_name().to_ascii(), "resolver.example.");
        assert!(
            dot.svc_params()
                .contains(&(SvcParamKey::Port, SvcParamValue::Port(8853)))
        );
        let RData::SVCB(doh) = response.answers()[1].data() else {
            panic!("missing SVCB")
        };
        assert!(doh.svc_params().contains(&(
            SvcParamKey::Unknown(7),
            SvcParamValue::Unknown(Unknown(b"/dns-query{?dns}".to_vec()))
        )));
        assert_eq!(
            handler
                .lookup("resolver.example.", RecordType::A)
                .await
                .unwrap()
                .ip_addrs(),
            vec!["192.0.2.53".parse::<IpAddr>().unwrap()]
        );
        for name in ["_dns.resolver.arpa.", "unrelated.resolver.arpa."] {
            let response = handler.lookup(name, RecordType::AAAA).await.unwrap();
            assert!(response.answers().is_empty());
            assert_eq!(response.authorities()[0].record_type(), RecordType::SOA);
        }
    }
}
