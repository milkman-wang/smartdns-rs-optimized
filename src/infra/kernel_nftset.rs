//! Native Rust netlink support for nftables address sets.
use std::{io, net::IpAddr};

fn attribute(kind: u16, value: &[u8]) -> Vec<u8> {
    let length = 4 + value.len();
    let mut bytes = Vec::with_capacity((length + 3) & !3);
    bytes.extend((length as u16).to_ne_bytes());
    bytes.extend(kind.to_ne_bytes());
    bytes.extend(value);
    bytes.resize((length + 3) & !3, 0);
    bytes
}

fn message(kind: u16, flags: u16, sequence: u32, family: u8, attrs: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(20 + attrs.len());
    bytes.extend((20u32 + attrs.len() as u32).to_ne_bytes());
    bytes.extend(kind.to_ne_bytes());
    bytes.extend(flags.to_ne_bytes());
    bytes.extend(sequence.to_ne_bytes());
    bytes.extend(0u32.to_ne_bytes());
    bytes.extend([family, 0, 0, 0]);
    bytes.extend(attrs);
    bytes
}

fn family(name: &str) -> io::Result<u8> {
    match name {
        "inet" => Ok(1),
        "ip" => Ok(2),
        "arp" => Ok(3),
        "netdev" => Ok(5),
        "bridge" => Ok(7),
        "ip6" => Ok(10),
        "decnet" => Ok(12),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown nftables family",
        )),
    }
}

fn set_attributes(table: &str, set: &str) -> io::Result<Vec<u8>> {
    if [table, set]
        .iter()
        .any(|name| name.is_empty() || name.len() >= 256 || name.contains('\0'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid nftables table/set name",
        ));
    }
    let mut attrs = attribute(1, &[table.as_bytes(), &[0]].concat());
    attrs.extend(attribute(2, &[set.as_bytes(), &[0]].concat()));
    Ok(attrs)
}

fn set_flags(reply: &[u8]) -> io::Result<u32> {
    let mut attrs = reply
        .get(4..)
        .ok_or_else(|| io::Error::other("missing nftables set metadata"))?;
    while attrs.len() >= 4 {
        let length = u16::from_ne_bytes(attrs[..2].try_into().unwrap()) as usize;
        let kind = u16::from_ne_bytes(attrs[2..4].try_into().unwrap()) & 0x3fff;
        if length < 4 || length > attrs.len() {
            break;
        }
        if kind == 3 && length == 8 {
            return Ok(u32::from_be_bytes(attrs[4..8].try_into().unwrap()));
        }
        let aligned = (length + 3) & !3;
        attrs = attrs.get(aligned..).unwrap_or_default();
    }
    Err(io::Error::other("nftables response omits set flags"))
}

fn elements(ip: IpAddr, flags: u32, timeout: u32) -> io::Result<Vec<u8>> {
    let address = match ip {
        IpAddr::V4(ip) => ip.octets().to_vec(),
        IpAddr::V6(ip) => ip.octets().to_vec(),
    };
    let mut element = attribute(0x8001, &attribute(1, &address));
    if flags & 16 != 0 && timeout > 0 {
        element.extend(attribute(4, &(u64::from(timeout) * 1000).to_be_bytes()));
    }
    let mut list = attribute(0x8001, &element);
    if flags & 4 != 0 {
        let mut end = address;
        let mut carried = true;
        for byte in end.iter_mut().rev() {
            let (next, overflow) = byte.overflowing_add(1);
            *byte = next;
            if !overflow {
                carried = false;
                break;
            }
        }
        if carried {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "nftables interval endpoint overflows",
            ));
        }
        let mut element = attribute(3, &1u32.to_be_bytes());
        element.extend(attribute(0x8001, &attribute(1, &end)));
        list.extend(attribute(0x8001, &element));
    }
    Ok(attribute(0x8003, &list))
}

fn batch(command: u16, family: u8, attrs: &[u8]) -> Vec<u8> {
    let mut begin = message(16, 1, 1, 0, &[]); // NFNL_MSG_BATCH_BEGIN
    begin[18..20].copy_from_slice(&10u16.to_be_bytes());
    begin.extend(message(
        (10 << 8) | command,
        1 | 4 | 0x400,
        2,
        family,
        attrs,
    ));
    let mut end = message(17, 1, 3, 0, &[]); // NFNL_MSG_BATCH_END
    end[18..20].copy_from_slice(&10u16.to_be_bytes());
    begin.extend(end);
    begin
}

#[cfg(all(feature = "nft", target_os = "linux"))]
async fn exchange(packet: &[u8], sequence: u32) -> io::Result<Vec<Vec<u8>>> {
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
    let mut peer: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    peer.nl_family = libc::AF_NETLINK as u16;
    if unsafe {
        libc::sendto(
            fd.as_raw_fd(),
            packet.as_ptr().cast(),
            packet.len(),
            0,
            (&peer as *const libc::sockaddr_nl).cast(),
            std::mem::size_of_val(&peer) as _,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        let mut messages = vec![];
        loop {
            let mut ready = fd.readable().await?;
            let received = ready.try_io(|fd| {
                let mut bytes = vec![0u8; 8192];
                let count = unsafe {
                    libc::recv(fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0)
                };
                if count < 0 {
                    return Err(io::Error::last_os_error());
                }
                bytes.truncate(count as usize);
                Ok(bytes)
            });
            let Ok(received) = received else {
                continue;
            };
            let bytes = received?;
            let mut remaining = bytes.as_slice();
            while remaining.len() >= 16 {
                let length = u32::from_ne_bytes(remaining[..4].try_into().unwrap()) as usize;
                if length < 16 || length > remaining.len() {
                    return Err(io::Error::other("invalid nftables netlink response"));
                }
                let kind = u16::from_ne_bytes(remaining[4..6].try_into().unwrap());
                let seq = u32::from_ne_bytes(remaining[8..12].try_into().unwrap());
                let payload = &remaining[16..length];
                if seq == sequence {
                    if kind == 2 {
                        let status = payload
                            .get(..4)
                            .ok_or_else(|| io::Error::other("missing netlink status"))?;
                        let status = i32::from_ne_bytes(status.try_into().unwrap());
                        if status != 0 {
                            return Err(io::Error::from_raw_os_error(-status));
                        }
                        return Ok(messages);
                    }
                    messages.push(payload.to_vec());
                }
                remaining = remaining.get(((length + 3) & !3)..).unwrap_or_default();
            }
        }
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "nftables acknowledgement timed out",
        )
    })?
}

pub async fn add(
    family_name: &str,
    table: &str,
    set: &str,
    ip: IpAddr,
    timeout: u32,
) -> io::Result<()> {
    let family = family(family_name)?;
    let mut attrs = set_attributes(table, set)?;
    #[cfg(all(feature = "nft", target_os = "linux"))]
    {
        let replies = exchange(&message((10 << 8) | 10, 1 | 4, 1, family, &attrs), 1).await?;
        let flags = set_flags(
            replies
                .first()
                .ok_or_else(|| io::Error::other("missing nftables set response"))?,
        )?;
        attrs.extend(elements(ip, flags, timeout)?);
        if timeout > 0 && flags & 16 != 0 {
            // An existing element's timeout is refreshed by replacing it.
            let mut deletion = set_attributes(table, set)?;
            deletion.extend(elements(ip, flags, 0)?);
            if let Err(error) = exchange(&batch(14, family, &deletion), 2).await
                && error.raw_os_error() != Some(libc::ENOENT)
            {
                return Err(error);
            }
        }
        exchange(&batch(12, family, &attrs), 2).await?;
        Ok(())
    }
    #[cfg(not(all(feature = "nft", target_os = "linux")))]
    {
        let _ = (&mut attrs, family, ip, timeout);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "nftset requires Linux and the nft feature",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_interval_end_and_timeout_encoding() {
        let bytes = elements("192.0.2.255".parse().unwrap(), 4 | 16, 90).unwrap();
        assert!(bytes.windows(4).any(|value| value == [192, 0, 2, 255]));
        assert!(bytes.windows(4).any(|value| value == [192, 0, 3, 0]));
        assert!(
            bytes
                .windows(8)
                .any(|value| value == 90000u64.to_be_bytes())
        );
        let ordinary = elements("192.0.2.255".parse().unwrap(), 0, 90).unwrap();
        assert!(
            !ordinary
                .windows(8)
                .any(|value| value == 90000u64.to_be_bytes())
        );
        assert!(!ordinary.windows(4).any(|value| value == [192, 0, 3, 0]));
        let mut reply = vec![1, 0, 0, 0];
        reply.extend(attribute(3, &20u32.to_be_bytes()));
        assert_eq!(set_flags(&reply).unwrap(), 20);
        assert!(set_flags(&[0, 0, 0, 0]).is_err());
    }
}
