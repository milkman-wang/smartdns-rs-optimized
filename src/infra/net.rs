//! Addresses of the local network interfaces.

use std::{io, net::IpAddr};

/// The IPv4 and IPv6 addresses of the local interfaces, each with its
/// interface name.
#[cfg(target_os = "linux")]
pub fn list_afinet_netifas() -> io::Result<Vec<(String, IpAddr)>> {
    use std::{
        ffi::CStr,
        net::{Ipv4Addr, Ipv6Addr},
        ptr,
    };

    let mut addrs: *mut libc::ifaddrs = ptr::null_mut();
    // SAFETY: getifaddrs fills `addrs` with a list that freeifaddrs releases
    // below; every entry is read before that.
    unsafe {
        if libc::getifaddrs(&mut addrs) != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut list = vec![];
        let mut entry = addrs;
        while let Some(ifa) = entry.as_ref() {
            entry = ifa.ifa_next;
            let Some(addr) = ifa.ifa_addr.as_ref() else {
                continue;
            };
            let ip = match i32::from(addr.sa_family) {
                libc::AF_INET => {
                    let addr = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    IpAddr::V4(Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)))
                }
                libc::AF_INET6 => {
                    let addr = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                    IpAddr::V6(Ipv6Addr::from(addr.sin6_addr.s6_addr))
                }
                _ => continue,
            };
            let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
            list.push((name, ip));
        }
        libc::freeifaddrs(addrs);
        Ok(list)
    }
}

/// The IPv4 and IPv6 addresses of the local interfaces, each with its
/// interface name.
#[cfg(not(target_os = "linux"))]
pub fn list_afinet_netifas() -> io::Result<Vec<(String, IpAddr)>> {
    local_ip_address::list_afinet_netifas().map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lists_the_loopback_addresses() {
        let list = list_afinet_netifas().unwrap();
        assert!(list.iter().any(|(_, ip)| ip.is_loopback()), "{list:?}");
        assert!(list.iter().all(|(name, _)| !name.is_empty()));
    }
}
