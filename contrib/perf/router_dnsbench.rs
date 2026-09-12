//! Linux loopback DNS benchmark and deterministic UDP upstream.
//! CLI and JSON fields match the previous benchmark tool.
use std::io;

fn question(id: u16, name: u32, domains: u32) -> Vec<u8> {
    let mut bytes = vec![0; 12];
    bytes[..2].copy_from_slice(&id.to_be_bytes());
    bytes[2] = 1;
    bytes[5] = 1;
    let label = format!("host{:03}", name % domains);
    bytes.push(label.len() as u8);
    bytes.extend(label.as_bytes());
    bytes.extend(b"\x05bench\x00\x00\x01\x00\x01");
    bytes
}

fn answer(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    // Upstreams may add EDNS even when the original client did not. Retain
    // only the question, so an OPT record cannot precede the answer section.
    let mut question_end = 12;
    loop {
        let label = *query.get(question_end)? as usize;
        question_end += 1;
        if label == 0 {
            break;
        }
        if label > 63 {
            return None;
        }
        question_end += label;
    }
    question_end += 4;
    let mut bytes = query.get(..question_end)?.to_vec();
    bytes[2..4].copy_from_slice(&[0x81, 0x80]);
    bytes[6..12].copy_from_slice(&[0, 1, 0, 0, 0, 0]);
    bytes.extend([0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0x0e, 0x10, 0, 4, 192, 0, 2, 1]);
    Some(bytes)
}

fn valid_answer(bytes: &[u8], expected: &[u8]) -> bool {
    bytes.len() >= expected.len() + 16
        && bytes[2] & 0x80 != 0
        && bytes[3] & 15 == 0
        && bytes[6..8] == [0, 1]
        && bytes[12..expected.len()] == expected[12..]
        && bytes[bytes.len() - 4..] == [192, 0, 2, 1]
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        fs,
        net::UdpSocket,
        os::fd::AsRawFd,
        time::{Duration, Instant},
    };

    fn cpu_ticks(pid: u32) -> Option<i64> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let fields = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .collect::<Vec<_>>();
        Some(fields.get(11)?.parse::<i64>().ok()? + fields.get(12)?.parse::<i64>().ok()?)
    }

    fn rss_kb(pid: u32) -> i64 {
        fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|status| {
                status.lines().find_map(|line| {
                    line.strip_prefix("VmRSS:")?
                        .split_whitespace()
                        .next()?
                        .parse()
                        .ok()
                })
            })
            .unwrap_or(-1)
    }

    pub fn run(args: &[String]) -> io::Result<bool> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected server PORT | client PORT SECONDS WINDOW PID [DOMAINS] [QPS]",
            )
        };
        let mode = args.get(1).ok_or_else(invalid)?;
        let port: u16 = args
            .get(2)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let socket = UdpSocket::bind(("127.0.0.1", if mode == "server" { port } else { 0 }))?;
        let buffer_size: libc::c_int = 4 * 1024 * 1024;
        unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVBUF,
                (&buffer_size as *const libc::c_int).cast(),
                std::mem::size_of_val(&buffer_size) as _,
            );
        }
        let mut buffer = [0u8; 4096];
        if mode == "server" {
            loop {
                let (count, peer) = socket.recv_from(&mut buffer[..4080])?;
                if let Some(reply) = answer(&buffer[..count]) {
                    socket.send_to(&reply, peer)?;
                }
            }
        }
        if mode != "client" {
            return Err(invalid());
        }
        let seconds: f64 = args
            .get(3)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let window: usize = args
            .get(4)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let pid: u32 = args
            .get(5)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let domains: u32 = args
            .get(6)
            .map(String::as_str)
            .unwrap_or("256")
            .parse()
            .map_err(|_| invalid())?;
        let rate: f64 = args
            .get(7)
            .map(String::as_str)
            .unwrap_or("0")
            .parse()
            .map_err(|_| invalid())?;
        if !(1..=1024).contains(&window)
            || domains == 0
            || !seconds.is_finite()
            || seconds <= 0.0
            || !rate.is_finite()
            || rate < 0.0
        {
            return Err(invalid());
        }
        socket.connect(("127.0.0.1", port))?;
        let mut sent: Vec<Option<(Instant, u32)>> = vec![None; 65536];
        let mut histogram = vec![0u64; 100001];
        let (mut sequence, mut active) = (0u32, 0usize);
        let (mut success, mut errors, mut timeouts) = (0u64, 0u64, 0u64);
        let mut total_latency = 0.0;
        let start_ticks = cpu_ticks(pid);
        let client_start_ticks = cpu_ticks(std::process::id());
        let start = Instant::now();
        let deadline = start + Duration::from_secs_f64(seconds);
        let mut next_send = start;
        while Instant::now() < deadline || active > 0 {
            let now = Instant::now();
            while now < deadline && active < window && (rate == 0.0 || now >= next_send) {
                let id = sequence as u16;
                sequence = sequence.wrapping_add(1);
                let bytes = question(id, sequence, domains);
                sent[id as usize] = Some((Instant::now(), sequence));
                socket.send(&bytes)?;
                active += 1;
                if rate > 0.0 {
                    next_send += Duration::from_secs_f64(1.0 / rate);
                }
            }
            let wait = if rate > 0.0 && active < window && now < deadline {
                next_send
                    .saturating_duration_since(Instant::now())
                    .as_secs_f64()
                    .mul_add(1000.0, 0.999)
                    .min(10.0) as i32
            } else {
                10
            };
            let mut fd = libc::pollfd {
                fd: socket.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut fd, 1, wait) };
            if ready < 0 {
                return Err(io::Error::last_os_error());
            }
            if ready > 0 {
                let count = socket.recv(&mut buffer)?;
                if count < 12 {
                    errors += 1;
                    continue;
                }
                let id = u16::from_be_bytes([buffer[0], buffer[1]]);
                if let Some((started, name)) = sent[id as usize].take() {
                    active -= 1;
                    let elapsed = started.elapsed().as_secs_f64() * 1e6;
                    if valid_answer(&buffer[..count], &question(id, name, domains)) {
                        success += 1;
                        total_latency += elapsed;
                        histogram[(elapsed as usize).min(100000)] += 1;
                    } else {
                        errors += 1;
                    }
                } else {
                    errors += 1;
                }
            } else if active > 0 {
                for item in &mut sent {
                    if item.is_some_and(|(sent, _)| sent.elapsed() > Duration::from_millis(500)) {
                        *item = None;
                        active -= 1;
                        timeouts += 1;
                    }
                }
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        let cpu = start_ticks
            .zip(cpu_ticks(pid))
            .map(|(start, end)| 100.0 * (end - start) as f64 / ticks_per_second / elapsed)
            .unwrap_or(-1.0);
        let client_cpu = client_start_ticks
            .zip(cpu_ticks(std::process::id()))
            .map(|(start, end)| 100.0 * (end - start) as f64 / ticks_per_second / elapsed)
            .unwrap_or(-1.0);
        let percentile = |fraction: f64| {
            let wanted = (success as f64 * fraction).ceil() as u64;
            let mut cumulative = 0;
            histogram
                .iter()
                .position(|count| {
                    cumulative += count;
                    cumulative >= wanted
                })
                .unwrap_or(100000)
        };
        println!(
            "{{\"qps\":{:.1},\"success\":{},\"errors\":{},\"timeouts\":{},\"seconds\":{:.3},\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"avg_us\":{:.1},\"server_cpu_pct\":{:.1},\"client_cpu_pct\":{:.1},\"server_rss_kb\":{}}}",
            success as f64 / elapsed,
            success,
            errors,
            timeouts,
            elapsed,
            percentile(0.5),
            percentile(0.95),
            percentile(0.99),
            if success > 0 {
                total_latency / success as f64
            } else {
                0.0
            },
            cpu,
            client_cpu,
            rss_kb(pid)
        );
        Ok(errors == 0 && timeouts == 0)
    }
}

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    match linux::run(&std::env::args().collect::<Vec<_>>()) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::from(2)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = io::ErrorKind::Unsupported;
        eprintln!("router_dnsbench requires Linux");
        std::process::ExitCode::from(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn benchmark_wire_query_and_reply() {
        let query = question(0x1234, 257, 256);
        assert_eq!(&query[..6], &[0x12, 0x34, 1, 0, 0, 1]);
        assert!(query.windows(7).any(|value| value == b"host001"));
        let mut reply = answer(&query).unwrap();
        assert!(valid_answer(&reply, &query));
        *reply.last_mut().unwrap() = 2;
        assert!(!valid_answer(&reply, &query));
    }

    #[test]
    fn benchmark_upstream_accepts_edns() {
        let query = question(0x1234, 257, 256);
        let mut with_edns = query.clone();
        with_edns[11] = 1;
        with_edns.extend([0, 0, 41, 4, 208, 0, 0, 0, 0, 0, 0]);
        let reply = answer(&with_edns).unwrap();
        assert_eq!(reply.len(), query.len() + 16);
        assert_eq!(&reply[10..12], &[0, 0]);
        assert!(valid_answer(&reply, &query));
    }
}
