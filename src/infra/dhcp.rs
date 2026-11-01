//! Finds the DNS servers of a network by a DHCPv4 discovery.

use crate::log;
use anyhow::{Result, anyhow, bail};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::{sleep, timeout};

const TIMEOUT: Duration = Duration::from_secs(3);

const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

/// The options a discovery asks for: subnet mask, router, DNS servers,
/// domain name, router discovery, static routes, NetBIOS name servers,
/// node type and scope, domain search, classless static routes,
/// broadcast address, time offset and host name.
const PARAMETERS: [u8; 14] = [1, 3, 6, 15, 31, 33, 44, 46, 47, 119, 121, 28, 2, 12];

/// The parts of a network interface a DHCPv4 discovery needs.
pub struct NetworkInterface {
    pub name: String,
    pub mac_addr: Option<[u8; 6]>,
    /// The IPv4 gateway of its default route.
    pub gateway: Option<Ipv4Addr>,
}

/// The named interface, or the one with the default route.
pub fn find_interface_by_name(name: Option<&str>) -> Result<NetworkInterface> {
    platform::find_interface_by_name(name)
}

#[cfg(target_os = "linux")]
mod platform {
    //! Reads sysfs and the routing table directly.

    use super::NetworkInterface;
    use anyhow::{Result, anyhow};
    use std::net::Ipv4Addr;

    /// The default IPv4 routes in a /proc/net/route table, lowest metric
    /// first: interface and gateway (unspecified for none).
    pub(super) fn default_routes(table: &str) -> Vec<(String, Ipv4Addr)> {
        let mut routes = table
            .lines()
            .skip(1)
            .filter_map(|line| {
                // Iface Destination Gateway Flags RefCnt Use Metric Mask ...
                let fields = line.split_whitespace().collect::<Vec<_>>();
                if fields.len() < 8 || fields[1] != "00000000" || fields[7] != "00000000" {
                    return None;
                }
                // The kernel prints the address in host byte order.
                let gateway = u32::from_str_radix(fields[2], 16).ok()?;
                let metric = fields[6].parse::<u32>().ok()?;
                Some((
                    metric,
                    fields[0].to_string(),
                    Ipv4Addr::from(gateway.to_ne_bytes()),
                ))
            })
            .collect::<Vec<_>>();
        routes.sort_by_key(|(metric, ..)| *metric);
        routes
            .into_iter()
            .map(|(_, iface, gateway)| (iface, gateway))
            .collect()
    }

    pub(super) fn parse_mac(text: &str) -> Option<[u8; 6]> {
        let mut mac = [0u8; 6];
        let mut parts = text.trim().split(':');
        for byte in &mut mac {
            *byte = u8::from_str_radix(parts.next()?, 16).ok()?;
        }
        (parts.next().is_none() && mac != [0; 6]).then_some(mac)
    }

    pub fn find_interface_by_name(name: Option<&str>) -> Result<NetworkInterface> {
        let routes =
            default_routes(&std::fs::read_to_string("/proc/net/route").unwrap_or_default());
        let name = match name {
            Some(name) if std::path::Path::new("/sys/class/net").join(name).exists() => {
                name.to_string()
            }
            Some(name) => return Err(anyhow!("Interface {} not found", name)),
            None => routes
                .first()
                .map(|(iface, _)| iface.clone())
                .ok_or_else(|| anyhow!("No default interface found"))?,
        };
        let gateway = routes
            .iter()
            .find(|(iface, gateway)| *iface == name && !gateway.is_unspecified())
            .map(|(_, gateway)| *gateway);
        let mac_addr = std::fs::read_to_string(format!("/sys/class/net/{name}/address"))
            .ok()
            .and_then(|text| parse_mac(&text));
        Ok(NetworkInterface {
            name,
            mac_addr,
            gateway,
        })
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::NetworkInterface;
    use anyhow::{Result, anyhow};

    pub fn find_interface_by_name(name: Option<&str>) -> Result<NetworkInterface> {
        let iface = match name {
            Some(name) => netdev::get_interfaces()
                .into_iter()
                .find(|iface| iface.name == name)
                .ok_or(anyhow!("Interface {} not found", name))?,
            None => netdev::get_default_interface().map_err(|err| anyhow!(err))?,
        };
        Ok(NetworkInterface {
            gateway: iface
                .gateway
                .iter()
                .flat_map(|gateway| gateway.ipv4.iter())
                .next()
                .copied(),
            mac_addr: iface.mac_addr.map(|mac| mac.octets()),
            name: iface.name,
        })
    }
}

/// A DHCPDISCOVER from `mac`.
fn dhcp_v4_discover(mac: [u8; 6], xid: u32, broadcast: bool, hostname: Option<&str>) -> Vec<u8> {
    let mut msg = vec![0u8; 236];
    msg[0] = 1; // BOOTREQUEST
    msg[1] = 1; // Ethernet
    msg[2] = 6; // hardware address length
    msg[4..8].copy_from_slice(&xid.to_be_bytes());
    if broadcast {
        msg[10] = 0x80;
    }
    msg[28..34].copy_from_slice(&mac);
    msg.extend_from_slice(&MAGIC_COOKIE);
    msg.extend_from_slice(&[53, 1, 1]); // message type: DISCOVER
    if let Some(host) = hostname.map(str::as_bytes).filter(|host| !host.is_empty()) {
        let host = &host[..host.len().min(255)];
        msg.extend_from_slice(&[12, host.len() as u8]);
        msg.extend_from_slice(host);
    }
    msg.extend_from_slice(&[61, 7, 1]); // client identifier: Ethernet, MAC
    msg.extend_from_slice(&mac);
    msg.extend_from_slice(&[55, PARAMETERS.len() as u8]);
    msg.extend_from_slice(&PARAMETERS);
    msg.push(255);
    msg
}

/// The DNS servers of a DHCPv4 reply.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DhcpV4Reply {
    domain_name_servers: Vec<Ipv4Addr>,
    name_servers: Vec<Ipv4Addr>,
}

impl DhcpV4Reply {
    fn parse(packet: &[u8], xid: u32) -> Result<Self> {
        if packet.len() < 240 || packet[0] != 2 || packet[236..240] != MAGIC_COOKIE {
            bail!("not a DHCPv4 reply");
        }
        if packet[4..8] != xid.to_be_bytes() {
            bail!("DHCPv4 reply to another request");
        }
        let mut reply = Self::default();
        let mut options = &packet[240..];
        while let Some((&code, rest)) = options.split_first() {
            match code {
                0 => {
                    options = rest;
                    continue;
                }
                255 => break,
                _ => {}
            }
            let Some((&len, rest)) = rest.split_first() else {
                bail!("truncated DHCPv4 option {code}");
            };
            if rest.len() < len as usize {
                bail!("truncated DHCPv4 option {code}");
            }
            let (value, rest) = rest.split_at(len as usize);
            let addrs = value
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&addr| Ipv4Addr::from(addr));
            match code {
                6 => reply.domain_name_servers.extend(addrs),
                5 => reply.name_servers.extend(addrs),
                _ => {}
            }
            options = rest;
        }
        Ok(reply)
    }
}

async fn exchange(
    mut bind_addr: SocketAddr,
    dst_addr: SocketAddr,
    xid: u32,
    message: &[u8],
) -> Result<DhcpV4Reply> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(
        Domain::for_address(bind_addr),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;

    socket.set_nonblocking(true)?;

    socket.set_reuse_address(true)?;

    if socket.bind(&bind_addr.into()).is_err() {
        bind_addr.set_port(0);
        socket.bind(&bind_addr.into())?;
    }

    let udp_socket = UdpSocket::from_std(std::net::UdpSocket::from(socket))?;

    if matches!(dst_addr.ip(), IpAddr::V4(ip) if ip.is_broadcast()) {
        udp_socket.set_broadcast(true)?;
    }

    udp_socket.send_to(message, dst_addr).await?;

    let mut buf = vec![0; 1500];
    loop {
        let (n, _) = udp_socket.recv_from(&mut buf[..]).await?;
        match DhcpV4Reply::parse(&buf[..n], xid) {
            Ok(reply) => return Ok(reply),
            Err(err) => log::debug!("ignoring DHCPv4 packet: {}", err),
        }
    }
}

pub async fn discover_v4(eth_name: Option<&str>) -> Result<DhcpV4Reply> {
    let interface = find_interface_by_name(eth_name)?;
    let mac_addr = interface
        .mac_addr
        .ok_or_else(|| anyhow!("No MAC address found for interface"))?;

    let gateway = interface.gateway.unwrap_or(Ipv4Addr::BROADCAST);

    let mut retry_count = 3;
    while retry_count > 0 {
        match send_discover_v4(mac_addr, gateway).await {
            Ok(msg) => return Ok(msg),
            Err(e) => {
                log::error!("Failed to discover DHCPv4 server: {}", e);
                retry_count -= 1;
                if retry_count == 0 {
                    return Err(e);
                }
                sleep(Duration::from_secs(1)).await; // Wait for a second before retrying
            }
        }
    }
    Err(anyhow!("Failed to discover DHCPv4 server after retries"))
}

pub async fn send_discover_v4(mac_addr: [u8; 6], dst: Ipv4Addr) -> Result<DhcpV4Reply> {
    let xid = rand::random();
    let hostname = hostname::get()
        .ok()
        .and_then(|host| host.into_string().ok());
    let msg = dhcp_v4_discover(mac_addr, xid, false, hostname.as_deref());
    let src = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 68);
    let dst = SocketAddr::new(dst.into(), 67);
    timeout(TIMEOUT, exchange(src, dst, xid, &msg)).await?
}

pub trait DhcpMessageExt {
    fn nameservers(&self) -> Vec<IpAddr>;
}

impl DhcpMessageExt for DhcpV4Reply {
    fn nameservers(&self) -> Vec<IpAddr> {
        let servers = if self.domain_name_servers.is_empty() {
            &self.name_servers
        } else {
            &self.domain_name_servers
        };
        servers.iter().map(|&ip| ip.into()).collect()
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    const MAC: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

    #[test]
    fn test_discover_encoding() {
        let msg = dhcp_v4_discover(MAC, 0x1234_5678, true, Some("router"));
        assert_eq!(&msg[..4], &[1, 1, 6, 0]);
        assert_eq!(&msg[4..8], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(&msg[10..12], &[0x80, 0]);
        assert_eq!(&msg[28..34], &MAC);
        assert_eq!(&msg[236..240], &MAGIC_COOKIE);
        let mut options = vec![53, 1, 1, 12, 6];
        options.extend_from_slice(b"router");
        options.extend_from_slice(&[61, 7, 1]);
        options.extend_from_slice(&MAC);
        options.extend_from_slice(&[
            55, 14, 1, 3, 6, 15, 31, 33, 44, 46, 47, 119, 121, 28, 2, 12, 255,
        ]);
        assert_eq!(&msg[240..], &options[..]);

        let msg = dhcp_v4_discover(MAC, 1, false, Some(""));
        assert_eq!(msg[10], 0);
        assert_eq!(&msg[240..246], &[53, 1, 1, 61, 7, 1]);
    }

    fn reply(xid: u32, options: &[u8]) -> Vec<u8> {
        let mut packet = vec![0u8; 236];
        packet[0] = 2;
        packet[4..8].copy_from_slice(&xid.to_be_bytes());
        packet.extend_from_slice(&MAGIC_COOKIE);
        packet.extend_from_slice(options);
        packet
    }

    #[test]
    fn test_reply_parsing() {
        let packet = reply(
            7,
            &[
                53, 1, 2, 0, 6, 8, 192, 168, 1, 1, 8, 8, 8, 8, 5, 4, 10, 0, 0, 1, 255,
            ],
        );
        let parsed = DhcpV4Reply::parse(&packet, 7).unwrap();
        assert_eq!(
            parsed.nameservers(),
            ["192.168.1.1", "8.8.8.8"].map(|ip| ip.parse::<IpAddr>().unwrap())
        );

        // Name servers (option 5) answer when DNS servers (option 6) are absent.
        let packet = reply(7, &[5, 4, 10, 0, 0, 1, 255]);
        assert_eq!(
            DhcpV4Reply::parse(&packet, 7).unwrap().nameservers(),
            ["10.0.0.1".parse::<IpAddr>().unwrap()]
        );

        assert!(DhcpV4Reply::parse(&reply(8, &[255]), 7).is_err());
        assert!(DhcpV4Reply::parse(&reply(7, &[6, 8, 192, 168]), 7).is_err());
        assert!(DhcpV4Reply::parse(&packet[..239], 7).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_routes_and_mac() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
            wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
            eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
            eth0\t0000000A\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        let routes = platform::default_routes(table);
        let native =
            |hex: &str| Ipv4Addr::from(u32::from_str_radix(hex, 16).unwrap().to_ne_bytes());
        assert_eq!(
            routes,
            [
                ("eth0".to_string(), native("0100000A")),
                ("wlan0".to_string(), native("0101A8C0"))
            ]
        );
        if cfg!(target_endian = "little") {
            assert_eq!(routes[0].1, Ipv4Addr::new(10, 0, 0, 1));
        }

        assert_eq!(platform::parse_mac("02:11:22:33:44:55\n"), Some(MAC));
        assert_eq!(platform::parse_mac("00:00:00:00:00:00"), None);
        assert_eq!(platform::parse_mac("02:11:22"), None);
    }

    #[ignore]
    #[tokio::test]
    async fn test_dhcp_get_dns() -> anyhow::Result<()> {
        let msg = discover_v4(None).await?;
        assert!(!msg.nameservers().is_empty());
        Ok(())
    }
}
