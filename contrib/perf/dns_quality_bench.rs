//! dns_quality_bench: compare DNS servers on cold-query latency, cached-answer latency,
//! and how fast the address they hand out actually is (TCP 443 handshake).
//!
//! usage: dns_quality_bench <domains.txt> <out.tsv> <label=ip:port>...
//!
//! Every name is sent to all servers at the same moment, so upstream caches
//! favour none of them. The cached answer is read 5 s later, then each
//! address seen is timed with the best of three TCP 443 handshakes.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_ID: AtomicU16 = AtomicU16::new(0x1234);

#[derive(Clone, Default)]
struct Answer {
    ms: Option<f64>,
    ips: Vec<Ipv4Addr>,
}

fn build_query(id: u16, name: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // QD=1
    for label in name.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]); // root, type A, class IN
    q
}

fn skip_name(buf: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *buf.get(pos)? as usize;
        if len == 0 {
            return Some(pos + 1);
        }
        if len & 0xC0 == 0xC0 {
            return Some(pos + 2);
        }
        pos += 1 + len;
    }
}

fn parse_a(buf: &[u8], id: u16) -> Option<Vec<Ipv4Addr>> {
    if buf.len() < 12 || u16::from_be_bytes([buf[0], buf[1]]) != id {
        return None;
    }
    let qd = u16::from_be_bytes([buf[4], buf[5]]);
    let an = u16::from_be_bytes([buf[6], buf[7]]);
    let mut pos = 12;
    for _ in 0..qd {
        pos = skip_name(buf, pos)? + 4;
    }
    let mut ips = vec![];
    for _ in 0..an {
        pos = skip_name(buf, pos)?;
        let rtype = u16::from_be_bytes([*buf.get(pos)?, *buf.get(pos + 1)?]);
        let rdlen = u16::from_be_bytes([*buf.get(pos + 8)?, *buf.get(pos + 9)?]) as usize;
        pos += 10;
        if rtype == 1 && rdlen == 4 {
            let b = buf.get(pos..pos + 4)?;
            ips.push(Ipv4Addr::new(b[0], b[1], b[2], b[3]));
        }
        pos += rdlen;
    }
    Some(ips)
}

fn query(server: SocketAddr, name: &str) -> Answer {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind");
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let pkt = build_query(id, name);
    let start = Instant::now();
    if sock.send_to(&pkt, server).is_err() {
        return Answer::default();
    }
    let mut buf = [0u8; 4096];
    while let Ok((n, _)) = sock.recv_from(&mut buf) {
        if let Some(ips) = parse_a(&buf[..n], id) {
            return Answer {
                ms: Some(start.elapsed().as_secs_f64() * 1000.0),
                ips,
            };
        }
    }
    Answer::default()
}

/// Fastest of three TCP handshakes to ip:443, in milliseconds.
fn tcp_rtt(ip: Ipv4Addr) -> Option<f64> {
    let addr = SocketAddr::new(IpAddr::V4(ip), 443);
    let mut best: Option<f64> = None;
    for _ in 0..3 {
        let t = Instant::now();
        if TcpStream::connect_timeout(&addr, Duration::from_millis(1500)).is_ok() {
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            best = Some(best.map_or(ms, |b: f64| b.min(ms)));
        }
    }
    best
}

/// Queries every server for `name` at the same moment.
fn query_all(targets: &[(String, SocketAddr)], name: &str) -> Vec<Answer> {
    let handles: Vec<_> = targets
        .iter()
        .map(|(_, addr)| {
            let (addr, name) = (*addr, name.to_owned());
            thread::spawn(move || query(addr, &name))
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let domains: Vec<String> = fs::read_to_string(&args[1])
        .expect("domains file")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    let targets: Vec<(String, SocketAddr)> = args[3..]
        .iter()
        .map(|a| {
            let (label, addr) = a.split_once('=').expect("label=ip:port");
            (label.to_owned(), addr.parse().expect("ip:port"))
        })
        .collect();

    eprintln!("phase 1: cold queries for {} domains", domains.len());
    let cold: Vec<Vec<Answer>> = domains
        .iter()
        .map(|d| {
            let r = query_all(&targets, d);
            thread::sleep(Duration::from_millis(150));
            r
        })
        .collect();

    eprintln!("waiting 5 s for background refreshes");
    thread::sleep(Duration::from_secs(5));

    eprintln!("phase 2: cached queries");
    let warm: Vec<Vec<Answer>> = domains.iter().map(|d| query_all(&targets, d)).collect();

    let mut all_ips = BTreeSet::new();
    for answers in cold.iter().chain(warm.iter()) {
        for a in answers {
            all_ips.extend(a.ips.iter().copied());
        }
    }
    eprintln!("phase 3: TCP 443 handshake to {} addresses", all_ips.len());
    let queue = Arc::new(Mutex::new(all_ips.into_iter().collect::<Vec<_>>()));
    let rtts = Arc::new(Mutex::new(HashMap::new()));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let (queue, rtts) = (queue.clone(), rtts.clone());
            thread::spawn(move || {
                loop {
                    let Some(ip) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let rtt = tcp_rtt(ip);
                    rtts.lock().unwrap().insert(ip, rtt);
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    let rtts = rtts.lock().unwrap().clone();

    let mut out = fs::File::create(&args[2]).expect("out file");
    writeln!(
        out,
        "domain\ttarget\tcold_ms\tcold_n\twarm_ms\twarm_n\twarm_first_ip\twarm_first_rtt\tbest_rtt"
    )
    .unwrap();

    struct Stats {
        cold: Vec<f64>,
        cold_fail: usize,
        warm: Vec<f64>,
        warm_n: Vec<f64>,
        penalty: Vec<f64>,
        first_rtt: Vec<f64>,
        best_hits: usize,
        compared: usize,
    }
    let mut stats: BTreeMap<usize, Stats> = BTreeMap::new();
    for (i, d) in domains.iter().enumerate() {
        // Best reachable address any server returned for this domain.
        let seen: BTreeSet<Ipv4Addr> = cold[i]
            .iter()
            .chain(warm[i].iter())
            .flat_map(|a| a.ips.iter().copied())
            .collect();
        let best = seen
            .iter()
            .filter_map(|ip| rtts.get(ip).copied().flatten())
            .fold(f64::INFINITY, f64::min);
        for (t, (label, _)) in targets.iter().enumerate() {
            let s = stats.entry(t).or_insert_with(|| Stats {
                cold: vec![],
                cold_fail: 0,
                warm: vec![],
                warm_n: vec![],
                penalty: vec![],
                first_rtt: vec![],
                best_hits: 0,
                compared: 0,
            });
            let (c, w) = (&cold[i][t], &warm[i][t]);
            match c.ms {
                Some(ms) => s.cold.push(ms),
                None => s.cold_fail += 1,
            }
            if let Some(ms) = w.ms {
                s.warm.push(ms);
                s.warm_n.push(w.ips.len() as f64);
            }
            let first = w.ips.first().copied();
            let first_rtt = first.and_then(|ip| rtts.get(&ip).copied().flatten());
            // Only domains with a choice of addresses say anything about selection.
            if seen.len() > 1
                && best.is_finite()
                && let Some(r) = first_rtt
            {
                s.first_rtt.push(r);
                s.penalty.push(r - best);
                s.compared += 1;
                if r - best <= (best * 0.10).max(1.0) {
                    s.best_hits += 1;
                }
            }
            writeln!(
                out,
                "{d}\t{label}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                c.ms.map_or("x".into(), |v| format!("{v:.1}")),
                c.ips.len(),
                w.ms.map_or("x".into(), |v| format!("{v:.2}")),
                w.ips.len(),
                first.map_or("-".into(), |ip| ip.to_string()),
                first_rtt.map_or("x".into(), |v| format!("{v:.1}")),
                if best.is_finite() {
                    format!("{best:.1}")
                } else {
                    "x".into()
                },
            )
            .unwrap();
        }
    }

    println!(
        "{:<22} {:>6} {:>7} {:>7} {:>7} {:>7} {:>5} | {:>8} {:>6} | {:>6} {:>8} {:>8} {:>9}",
        "server",
        "cold n",
        "p50",
        "p90",
        "p99",
        "mean",
        "fail",
        "warm p50",
        "IPs",
        "picks",
        "rtt p50",
        "penalty",
        "best-hit"
    );
    for (t, (label, _)) in targets.iter().enumerate() {
        let s = stats.get_mut(&t).unwrap();
        for v in [&mut s.cold, &mut s.warm, &mut s.first_rtt, &mut s.penalty] {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        }
        let mean = |v: &Vec<f64>| v.iter().sum::<f64>() / v.len().max(1) as f64;
        println!(
            "{:<22} {:>6} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>5} | {:>8.2} {:>6.2} | {:>6} {:>8.1} {:>8.1} {:>8.0}%",
            label,
            s.cold.len(),
            pct(&s.cold, 0.5),
            pct(&s.cold, 0.9),
            pct(&s.cold, 0.99),
            mean(&s.cold),
            s.cold_fail,
            pct(&s.warm, 0.5),
            mean(&s.warm_n),
            s.compared,
            pct(&s.first_rtt, 0.5),
            mean(&s.penalty),
            100.0 * s.best_hits as f64 / s.compared.max(1) as f64
        );
    }
}
