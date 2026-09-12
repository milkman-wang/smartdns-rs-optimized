use crate::{dns::*, middleware::*};
use std::{
    collections::HashMap,
    net::IpAddr,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

#[derive(Default)]
struct LeaseRecords {
    forward: HashMap<Name, Vec<IpAddr>>,
    reverse: HashMap<IpAddr, Name>,
}

impl LeaseRecords {
    fn parse(text: &str, zone: Option<&Name>) -> Self {
        let mut records = Self::default();
        for line in text.lines() {
            let line = line.trim();
            let (metadata, line) = match line.strip_prefix('#') {
                Some(rest) if rest.starts_with(char::is_whitespace) => (true, rest.trim_start()),
                Some(_) => continue,
                None => (false, line),
            };
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }
            if !metadata && let Ok(ip) = parts[0].parse::<IpAddr>() {
                for host in parts
                    .iter()
                    .skip(1)
                    .take_while(|host| !host.starts_with('#'))
                {
                    records.add(host, ip, zone);
                }
            } else if parts.len() >= 8 {
                for token in &parts[7..] {
                    if let Ok(ip) = token.split('/').next().unwrap().parse::<IpAddr>() {
                        records.add(parts[3], ip, zone);
                    }
                }
            }
        }
        records
    }

    fn add(&mut self, host: &str, ip: IpAddr, zone: Option<&Name>) {
        if matches!(host, "*" | "-" | "0") {
            return;
        }
        let Ok(mut name) = host.parse::<Name>() else {
            return;
        };
        name.set_fqdn(true);
        let canonical = if name.num_labels() == 1 {
            zone.and_then(|zone| name.clone().append_domain(zone).ok())
                .unwrap_or_else(|| name.clone())
        } else {
            name.clone()
        };
        for alias in [name, canonical.clone()] {
            let ips = self.forward.entry(alias).or_default();
            if !ips.contains(&ip) {
                ips.push(ip);
            }
        }
        self.reverse.entry(ip).or_insert(canonical);
    }
}

struct LeaseCache {
    checked: Instant,
    modified: Option<SystemTime>,
    records: LeaseRecords,
}

pub struct OdhcpdMiddleware {
    path: PathBuf,
    zone: Option<Name>,
    cache: Mutex<LeaseCache>,
}

impl OdhcpdMiddleware {
    pub fn new(path: PathBuf, zone: Option<Name>) -> Self {
        Self {
            path,
            zone,
            cache: Mutex::new(LeaseCache {
                checked: Instant::now() - Duration::from_secs(3),
                modified: None,
                records: LeaseRecords::default(),
            }),
        }
    }

    fn lookup(&self, name: &Name, qtype: RecordType) -> Vec<RData> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.checked.elapsed() >= Duration::from_secs(2) {
            cache.checked = Instant::now();
            let modified = std::fs::metadata(&self.path)
                .and_then(|m| m.modified())
                .ok();
            if modified != cache.modified {
                match std::fs::read_to_string(&self.path) {
                    Ok(text) => {
                        cache.records = LeaseRecords::parse(&text, self.zone.as_ref());
                        cache.modified = modified;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        cache.records = LeaseRecords::default();
                        cache.modified = None;
                    }
                    Err(error) => crate::log::warn!(
                        "cannot read odhcpd leases {}: {error}",
                        self.path.display()
                    ),
                }
            }
        }
        if qtype == RecordType::PTR {
            return crate::dnsmasq::ptr_to_ip(name)
                .ok()
                .and_then(|ip| cache.records.reverse.get(&ip))
                .map(|host| {
                    vec![RData::PTR(crate::libdns::proto::rr::rdata::PTR(
                        host.clone(),
                    ))]
                })
                .unwrap_or_default();
        }
        cache
            .records
            .forward
            .get(name)
            .into_iter()
            .flatten()
            .filter(|ip| {
                matches!(
                    (qtype, ip),
                    (RecordType::A, IpAddr::V4(_)) | (RecordType::AAAA, IpAddr::V6(_))
                )
            })
            .copied()
            .map(RData::from)
            .collect()
    }
}

#[async_trait::async_trait]
impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for OdhcpdMiddleware {
    async fn handle(
        &self,
        ctx: &mut DnsContext,
        req: &DnsRequest,
        next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
    ) -> Result<DnsResponse, DnsError> {
        let query = req.query().original();
        let data = self.lookup(query.name(), query.query_type());
        if data.is_empty() {
            return next.run(ctx, req).await;
        }
        ctx.source = LookupFrom::Static;
        let ttl = ctx.cfg().local_ttl() as u32;
        Ok(DnsResponse::new_with_max_ttl(
            query.clone(),
            data.into_iter()
                .map(|data| Record::from_rdata(query.name().clone(), ttl, data)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_odhcpd_and_hosts_records() {
        let records = LeaseRecords::parse(
            "# br-lan 00030001aabbccddeeff 1234 laptop -1 8 128 fd00::8/128\nfd00::9 laptop\n192.168.1.9 printer\n# br-lan ignored 1 * 0 0 128 fd00::10/128",
            Some(&"lan".parse().unwrap()),
        );
        assert_eq!(
            records.forward[&"laptop.lan.".parse::<Name>().unwrap()],
            vec![
                "fd00::8".parse::<IpAddr>().unwrap(),
                "fd00::9".parse().unwrap()
            ]
        );
        assert_eq!(
            records.reverse[&"fd00::8".parse::<IpAddr>().unwrap()].to_string(),
            "laptop.lan."
        );
        assert!(
            !records
                .reverse
                .contains_key(&"fd00::10".parse::<IpAddr>().unwrap())
        );
        assert_eq!(
            records.forward[&"printer.".parse::<Name>().unwrap()],
            vec!["192.168.1.9".parse::<IpAddr>().unwrap()]
        );
    }
}
