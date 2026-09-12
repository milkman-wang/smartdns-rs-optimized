use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

type MacEntries = HashMap<IpAddr, (Instant, Option<String>)>;
static CACHE: LazyLock<Mutex<MacEntries>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const REFRESH: Duration = Duration::from_secs(2);

/// Resolve a directly connected client's MAC without running a command on every query.
pub async fn lookup(ip: IpAddr) -> Option<String> {
    let ip = ip.to_canonical();
    if ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, value)) = cache.get(&ip)
            && at.elapsed() < REFRESH
        {
            return value.clone();
        }
    }
    tokio::task::spawn_blocking(move || {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, value)) = cache.get(&ip)
            && at.elapsed() < REFRESH
        {
            return value.clone();
        }
        let value = if ip.is_ipv4() {
            super::arp::lookup_client_mac_from_arp(ip)
        } else {
            lookup_neighbor(ip)
        };
        cache.insert(ip, (Instant::now(), value.clone()));
        value
    })
    .await
    .ok()
    .flatten()
}

fn lookup_neighbor(ip: IpAddr) -> Option<String> {
    let mut command = {
        #[cfg(target_os = "linux")]
        {
            let mut cmd = std::process::Command::new("ip");
            cmd.args(["-6", "neigh", "show", "to", &ip.to_string()]);
            cmd
        }
        #[cfg(target_os = "windows")]
        {
            let mut cmd = std::process::Command::new("netsh");
            cmd.args(["interface", "ipv6", "show", "neighbors"]);
            cmd
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let mut cmd = std::process::Command::new("ndp");
            cmd.args(["-n", &ip.to_string()]);
            cmd
        }
    };
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_neighbor(&String::from_utf8_lossy(&output.stdout), ip)
}

fn parse_neighbor(text: &str, ip: IpAddr) -> Option<String> {
    text.lines().find_map(|line| {
        let parts: Vec<_> = line.split_whitespace().collect();
        if !parts.iter().any(|token| {
            token
                .split('%')
                .next()
                .and_then(|s| s.parse::<IpAddr>().ok())
                == Some(ip)
        }) {
            return None;
        }
        parts.into_iter().find_map(|token| {
            token
                .replace('-', ":")
                .parse::<mac_addr::MacAddr>()
                .ok()
                .filter(|mac| *mac != mac_addr::MacAddr::zero())
                .map(|mac| mac.to_string())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbor_lookup_matches_the_whole_address() {
        let text = "fe80::10 dev br-lan lladdr aa:bb:cc:dd:ee:ff REACHABLE\nfe80::1%4 aa-bb-cc-dd-ee-01 Stale";
        assert_eq!(
            parse_neighbor(text, "fe80::1".parse().unwrap()).as_deref(),
            Some("aa:bb:cc:dd:ee:01")
        );
        assert_eq!(
            parse_neighbor(text, "fe80::10".parse().unwrap()).as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
        assert_eq!(parse_neighbor(text, "fe80::2".parse().unwrap()), None);
    }
}
