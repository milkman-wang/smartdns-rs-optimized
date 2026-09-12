//! Linux kernel IPSet protocol (distinct from the in-memory `ip-set` lists).
use std::{io, net::IpAddr};

fn attribute(kind: u16, data: &[u8]) -> Vec<u8> {
    let len = data.len() + 4;
    let mut attr = Vec::with_capacity((len + 3) & !3);
    attr.extend_from_slice(&(len as u16).to_ne_bytes());
    attr.extend_from_slice(&kind.to_ne_bytes());
    attr.extend_from_slice(data);
    attr.resize((len + 3) & !3, 0);
    attr
}

fn request(name: &str, ip: IpAddr, timeout: u32) -> io::Result<Vec<u8>> {
    if name.is_empty() || name.len() >= 32 || name.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid IPSet name",
        ));
    }
    let mut packet = vec![0; 16];
    packet[4..6].copy_from_slice(&((6u16 << 8) | 9).to_ne_bytes()); // IPSET_CMD_ADD
    packet[6..8].copy_from_slice(&(1u16 | 4 | 0x100).to_ne_bytes()); // REQUEST|ACK|REPLACE
    packet[8..12].copy_from_slice(&1u32.to_ne_bytes());
    packet.extend_from_slice(&[if ip.is_ipv4() { 2 } else { 10 }, 0, 0, 6]);
    packet.extend(attribute(1, &[6])); // protocol 6
    packet.extend(attribute(2, &[name.as_bytes(), &[0]].concat()));
    let addr = match ip {
        IpAddr::V4(ip) => attribute(0x4001, &ip.octets()),
        IpAddr::V6(ip) => attribute(0x4002, &ip.octets()),
    };
    let mut data = attribute(0x8001, &addr);
    if timeout > 0 {
        data.extend(attribute(0x4006, &timeout.to_be_bytes()));
    }
    packet.extend(attribute(0x8007, &data));
    let len = packet.len() as u32;
    packet[..4].copy_from_slice(&len.to_ne_bytes());
    Ok(packet)
}

pub async fn add(name: &str, ip: IpAddr, timeout: u32) -> io::Result<()> {
    let packet = request(name, ip, timeout)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let raw = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                libc::NETLINK_NETFILTER,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = tokio::io::unix::AsyncFd::new(unsafe { OwnedFd::from_raw_fd(raw) })?;
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as u16;
        let sent = unsafe {
            libc::sendto(
                fd.as_raw_fd(),
                packet.as_ptr().cast(),
                packet.len(),
                0,
                (&address as *const libc::sockaddr_nl).cast(),
                std::mem::size_of_val(&address) as _,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let mut ready = fd.readable().await?;
                let result = ready.try_io(|fd| {
                    let mut reply = [0u8; 1024];
                    let len = unsafe {
                        libc::recv(fd.as_raw_fd(), reply.as_mut_ptr().cast(), reply.len(), 0)
                    };
                    if len < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if len < 20 || u16::from_ne_bytes([reply[4], reply[5]]) != 2 {
                        return Err(io::Error::other("invalid IPSet acknowledgement"));
                    }
                    let status = i32::from_ne_bytes(reply[16..20].try_into().unwrap());
                    if status == 0 {
                        Ok(())
                    } else {
                        Err(io::Error::other(format!(
                            "IPSet rejected update (code {})",
                            -status
                        )))
                    }
                });
                if let Ok(result) = result {
                    return result;
                }
            }
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "IPSet acknowledgement timed out"))?
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = packet;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "kernel IPSet requires Linux",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires root and ipset"]
    async fn test_kernel_ipset_roundtrip() {
        use std::process::Command;
        let name = format!("smartdns_ipset_{}", std::process::id());
        let created = Command::new("ipset")
            .args(["create", &name, "hash:ip", "timeout", "120"])
            .output()
            .unwrap();
        assert!(
            created.status.success(),
            "{}",
            String::from_utf8_lossy(&created.stderr)
        );
        let result = add(&name, "192.0.2.4".parse().unwrap(), 90).await;
        let listing = Command::new("ipset")
            .args(["list", &name])
            .output()
            .unwrap();
        let _ = Command::new("ipset").args(["destroy", &name]).output();
        result.unwrap();
        let listing = String::from_utf8(listing.stdout).unwrap();
        assert!(listing.contains("192.0.2.4 timeout"), "{listing}");
        assert!(
            add("smartdns_absent", "192.0.2.4".parse().unwrap(), 0)
                .await
                .is_err()
        );
    }
    #[test]
    fn test_ipset_wire_address_and_timeout() {
        let packet = request("route4", "192.0.2.4".parse().unwrap(), 90).unwrap();
        assert_eq!(packet[16], 2);
        assert!(packet.windows(4).any(|v| v == [192, 0, 2, 4]));
        assert_eq!(&packet[packet.len() - 4..], &90u32.to_be_bytes());
        assert_eq!(
            u32::from_ne_bytes(packet[..4].try_into().unwrap()) as usize,
            packet.len()
        );
        let ip: IpAddr = "2001:db8::6".parse().unwrap();
        let packet = request("route6", ip, 0).unwrap();
        assert_eq!(packet[16], 10);
        let IpAddr::V6(ip) = ip else { unreachable!() };
        assert_eq!(&packet[packet.len() - 16..], &ip.octets());
    }
}
