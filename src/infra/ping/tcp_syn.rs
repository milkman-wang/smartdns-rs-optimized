//! Half-open TCP latency probes. Raw TCP requires CAP_NET_RAW on Linux.
use super::{PingAddr, PingError, PingOptions, PingOutput, do_agg};
use std::{net::SocketAddr, time::Duration};

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = bytes
        .chunks(2)
        .map(|p| u16::from_be_bytes([p[0], *p.get(1).unwrap_or(&0)]) as u32)
        .sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn tcp_packet(source: SocketAddr, dest: SocketAddr, seq: u32, ack: u32, flags: u8) -> [u8; 20] {
    use std::net::IpAddr;
    let mut tcp = [0u8; 20];
    tcp[..2].copy_from_slice(&source.port().to_be_bytes());
    tcp[2..4].copy_from_slice(&dest.port().to_be_bytes());
    tcp[4..8].copy_from_slice(&seq.to_be_bytes());
    tcp[8..12].copy_from_slice(&ack.to_be_bytes());
    tcp[12] = 5 << 4;
    tcp[13] = flags;
    tcp[14..16].copy_from_slice(&64240u16.to_be_bytes());
    let mut pseudo = vec![];
    match (source.ip(), dest.ip()) {
        (IpAddr::V4(src), IpAddr::V4(dst)) => {
            pseudo.extend(src.octets());
            pseudo.extend(dst.octets());
            pseudo.extend([0, 6, 0, 20]);
        }
        (IpAddr::V6(src), IpAddr::V6(dst)) => {
            pseudo.extend(src.octets());
            pseudo.extend(dst.octets());
            pseudo.extend([0, 0, 0, 20, 0, 0, 0, 6]);
        }
        _ => unreachable!("probe address families match"),
    }
    pseudo.extend(tcp);
    tcp[16..18].copy_from_slice(&checksum(&pseudo).to_be_bytes());
    tcp
}

// Raw IPv4 receives an IP header; Linux raw IPv6 starts at the TCP header.
fn reply_tcp(
    packet: &[u8],
    ipv4: bool,
    local_port: u16,
    remote_port: u16,
    seq: u32,
) -> Option<(bool, u32)> {
    let offset = if ipv4 {
        if packet.len() < 20 || packet[0] >> 4 != 4 || packet[9] != 6 {
            return None;
        }
        let ihl = (packet[0] & 15) as usize * 4;
        if ihl < 20 {
            return None;
        }
        ihl
    } else {
        0
    };
    let tcp = packet.get(offset..)?;
    if tcp.len() < 20
        || tcp[12] >> 4 < 5
        || tcp[..2] != remote_port.to_be_bytes()
        || tcp[2..4] != local_port.to_be_bytes()
        || tcp[13] & 0x10 == 0
        || tcp[8..12] != seq.wrapping_add(1).to_be_bytes()
    {
        return None;
    }
    if tcp[13] & 0x04 != 0 {
        return Some((false, 0));
    }
    if tcp[13] & 0x02 == 0 {
        return None;
    }
    Some((true, u32::from_be_bytes(tcp[4..8].try_into().unwrap())))
}

pub async fn ping(addr: SocketAddr, opts: PingOptions) -> Result<PingOutput, PingError> {
    let mut durations = vec![];
    let mut last_error = PingError::NoAddress;
    for _ in 0..opts.times {
        let result = tokio::time::timeout(opts.timeout, probe(addr))
            .await
            .unwrap_or(Err(PingError::Timeout));
        match result {
            Ok(duration) => durations.push(duration),
            Err(error) if opts.all_success => return Err(error),
            Err(error) => last_error = error,
        }
    }
    match do_agg(durations, opts.duration_agg) {
        Some(duration) => Ok(PingOutput {
            seq: 0,
            duration,
            destination: PingAddr::TcpSyn(addr),
        }),
        None => Err(last_error),
    }
}

#[cfg(target_os = "linux")]
async fn probe(dest: SocketAddr) -> Result<Duration, PingError> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::{io, net::SocketAddr, os::fd::AsRawFd, time::Instant};
    use tokio::io::unix::AsyncFd;

    let domain = Domain::for_address(dest);
    let route = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    route.connect(&dest.into())?;
    let local_ip = route.local_addr()?.as_socket().unwrap().ip();
    let reserved = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    reserved.bind(&SocketAddr::new(local_ip, 0).into())?;
    let source = reserved.local_addr()?.as_socket().unwrap();
    let socket = Socket::new(domain, Type::RAW, Some(Protocol::TCP))?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::new(local_ip, 0).into())?;
    let mut peer = dest;
    peer.set_port(0);
    socket.connect(&peer.into())?;
    let socket = AsyncFd::new(socket)?;
    let seq = rand::random::<u32>();
    let packet = tcp_packet(source, dest, seq, 0, 0x02);
    let start = Instant::now();
    loop {
        let mut ready = socket.writable().await?;
        if let Ok(result) = ready.try_io(|socket| socket.get_ref().send(&packet)) {
            result?;
            break;
        }
    }
    loop {
        let mut ready = socket.readable().await?;
        let result = ready.try_io(|socket| {
            let mut buffer = [0u8; 2048];
            let size = unsafe {
                libc::recv(
                    socket.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    0,
                )
            };
            if size < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(reply_tcp(
                &buffer[..size as usize],
                dest.is_ipv4(),
                source.port(),
                dest.port(),
                seq,
            ))
        });
        if let Ok(result) = result
            && let Some((open, peer_seq)) = result?
        {
            let elapsed = start.elapsed();
            // As in C SmartDNS, RST-ACK also proves the host is reachable.
            if !open {
                return Ok(elapsed);
            }
            // No final ACK: release the peer's SYN backlog entry immediately.
            let reset = tcp_packet(
                source,
                dest,
                seq.wrapping_add(1),
                peer_seq.wrapping_add(1),
                0x14,
            );
            let _ = socket.get_ref().send(&reset);
            return Ok(elapsed);
        }
    }
}

#[cfg(not(target_os = "linux"))]
async fn probe(_dest: SocketAddr) -> Result<Duration, PingError> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "TCP SYN probes require Linux raw sockets",
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syn_and_reply_matching() {
        let source: SocketAddr = "192.0.2.1:12345".parse().unwrap();
        let dest: SocketAddr = "192.0.2.2:443".parse().unwrap();
        let syn = tcp_packet(source, dest, 5, 0, 2);
        assert_eq!(syn[13], 2);
        let mut pseudo = vec![192, 0, 2, 1, 192, 0, 2, 2, 0, 6, 0, 20];
        pseudo.extend(syn);
        assert_eq!(checksum(&pseudo), 0);
        let mut ip_header = vec![0; 20];
        ip_header[0] = 0x45;
        ip_header[9] = 6;
        ip_header.extend(tcp_packet(dest, source, 100, 6, 0x12));
        assert_eq!(
            reply_tcp(&ip_header, true, 12345, 443, 5),
            Some((true, 100))
        );
        assert_eq!(reply_tcp(&ip_header, true, 12345, 443, 6), None);
        let source: SocketAddr = "[2001:db8::1]:12345".parse().unwrap();
        let dest: SocketAddr = "[2001:db8::2]:443".parse().unwrap();
        let reply = tcp_packet(dest, source, 100, 6, 0x12);
        assert_eq!(reply_tcp(&reply, false, 12345, 443, 5), Some((true, 100)));
        let reset = tcp_packet(dest, source, 0, 6, 0x14);
        assert_eq!(reply_tcp(&reset, false, 12345, 443, 5), Some((false, 0)));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires CAP_NET_RAW"]
    async fn test_syn_loopback_leaves_no_accepted_connection() {
        for address in ["127.0.0.1:0", "[::1]:0"] {
            let listener = tokio::net::TcpListener::bind(address).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let result = ping(addr, PingOptions::default()).await.unwrap();
            assert_eq!(result.dest(), PingAddr::TcpSyn(addr));
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        }
    }
}
