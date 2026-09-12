//! Optional packet diagnostics. The normal receive path only reads one flag.
use crate::libdns::proto::{
    ProtoError,
    op::Message,
    xfer::{DnsClientStream, SerialMessage},
};
use portable_atomic::AtomicU64;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

static ENABLED: AtomicBool = AtomicBool::new(false);
static DIRECTORY: RwLock<Option<PathBuf>> = RwLock::new(None);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn configure(enabled: bool, directory: Option<&Path>) {
    *DIRECTORY.write().unwrap() = Some(
        directory
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::temp_dir().join("smartdns")),
    );
    ENABLED.store(enabled, Ordering::Relaxed);
}

fn write_packet(
    directory: &Path,
    source: &str,
    address: SocketAddr,
    protocol: &str,
    bytes: &[u8],
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let name = format!(
        "{}-{}-{}-{}",
        source,
        address.to_string().replace([':', '[', ']'], "_"),
        chrono::Utc::now().timestamp_millis(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let path = directory.join(format!("{name}.bin"));
    std::fs::write(&path, bytes)?;
    std::fs::write(
        path.with_extension("txt"),
        format!(
            "source={source}\npeer={address}\nprotocol={protocol}\nlength={}\n",
            bytes.len()
        ),
    )?;
    Ok(path)
}

pub fn save(source: &str, address: SocketAddr, protocol: &str, bytes: &[u8]) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let directory = DIRECTORY.read().unwrap().clone();
    if let Some(directory) = directory
        && let Err(error) = write_packet(&directory, source, address, protocol, bytes)
    {
        crate::log::warn!("saving failed DNS packet: {error}");
    }
}

pub fn check_received(address: SocketAddr, protocol: &str, bytes: &[u8]) {
    if ENABLED.load(Ordering::Relaxed) && Message::from_vec(bytes).is_err() {
        save("client", address, protocol, bytes);
    }
}

pub struct InspectedStream<S> {
    stream: S,
    protocol: &'static str,
}

impl<S> InspectedStream<S> {
    pub fn new(stream: S, protocol: &'static str) -> Self {
        Self { stream, protocol }
    }
}

impl<S: std::fmt::Display> std::fmt::Display for InspectedStream<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.stream.fmt(f)
    }
}

impl<S: DnsClientStream + Unpin> DnsClientStream for InspectedStream<S> {
    type Time = S::Time;
    fn name_server_addr(&self) -> SocketAddr {
        self.stream.name_server_addr()
    }
}

impl<S: DnsClientStream + Unpin> futures::Stream for InspectedStream<S> {
    type Item = Result<SerialMessage, ProtoError>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let result = std::pin::Pin::new(&mut this.stream).poll_next(cx);
        if let std::task::Poll::Ready(Some(Ok(message))) = &result {
            check_received(message.addr(), this.protocol, message.bytes());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_saved_failure_contains_original_wire_bytes() {
        let dir =
            std::env::temp_dir().join(format!("smartdns-packet-test-{}", rand::random::<u64>()));
        let packet = [1u8, 2, 3];
        let path = write_packet(
            &dir,
            "server",
            "127.0.0.1:53".parse().unwrap(),
            "udp",
            &packet,
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), packet);
        let second = write_packet(
            &dir,
            "server",
            "127.0.0.1:53".parse().unwrap(),
            "udp",
            &[4, 5, 6],
        )
        .unwrap();
        assert_ne!(
            path, second,
            "repeated failures from one IPv4 peer must not overwrite each other"
        );
        assert_eq!(std::fs::read(&path).unwrap(), packet);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
