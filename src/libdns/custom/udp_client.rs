use super::connection_provider::TokioRuntimeProvider;
use crate::{
    dns::DnsResponse,
    libdns::{
        proto::{
            ProtoError, ProtoErrorKind,
            op::{
                Edns, Header, MessageSignature, MessageType, OpCode, Query,
                message::emit_message_parts,
            },
            rr::Record,
            runtime::RuntimeProvider,
            serialize::binary::BinEncoder,
            udp::{DnsUdpSocket, MAX_RECEIVE_BUFFER_SIZE},
            xfer::{DnsRequestOptions, DnsResponse as HickoryDnsResponse},
        },
        resolver::config::ResolverOpts,
    },
    proxy::UdpSocket,
};
use std::{
    borrow::Cow,
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
};

/// Reuses an idle UDP socket for one exchange at a time. Each in-flight query
/// owns its socket, so receiving needs no dispatcher task or completion queue.
/// Failed or cancelled exchanges drop their socket instead of returning it.
pub struct UdpClient {
    server: SocketAddr,
    options: Arc<ResolverOpts>,
    provider: TokioRuntimeProvider,
    idle: Mutex<Vec<ExchangeSocket>>,
}

struct ExchangeSocket {
    socket: UdpSocket,
    send: Vec<u8>,
    receive: Box<[u8; MAX_RECEIVE_BUFFER_SIZE]>,
}

impl UdpClient {
    const MAX_IDLE_SOCKETS: usize = 64;

    pub fn new(
        server: SocketAddr,
        options: Arc<ResolverOpts>,
        provider: TokioRuntimeProvider,
    ) -> Self {
        Self {
            server,
            options,
            provider,
            idle: Mutex::new(Vec::new()),
        }
    }

    pub async fn lookup(
        &self,
        query: &Query,
        options: DnsRequestOptions,
        edns: Option<&Edns>,
    ) -> Result<DnsResponse, ProtoError> {
        tokio::time::timeout(self.options.timeout, self.exchange(query, options, edns))
            .await
            .map_err(|_| ProtoError::from(ProtoErrorKind::Timeout))?
    }

    async fn exchange(
        &self,
        query: &Query,
        options: DnsRequestOptions,
        edns: Option<&Edns>,
    ) -> Result<DnsResponse, ProtoError> {
        let cached = self.idle.lock().unwrap().pop();
        let mut socket = match cached {
            Some(socket) => socket,
            None => ExchangeSocket {
                socket: self.bind().await?,
                send: Vec::with_capacity(512),
                receive: Box::new([0; MAX_RECEIVE_BUFFER_SIZE]),
            },
        };
        let mut query = Cow::Borrowed(query);
        if !query.name().is_fqdn() {
            query.to_mut().name.set_fqdn(true);
        }
        let original_query = if options.case_randomization {
            let original = query.clone().into_owned();
            query.to_mut().name.randomize_label_case();
            Some(original)
        } else {
            None
        };
        let id = rand::random();
        let mut header = Header::new(id, MessageType::Query, OpCode::Query);
        header.set_recursion_desired(options.recursion_desired);
        let receive_size =
            MAX_RECEIVE_BUFFER_SIZE.min(edns.map_or(512, Edns::max_payload) as usize);
        socket.send.clear();
        // A named lookup has one question and an optional OPT record. Encode
        // those parts directly instead of allocating an intermediate Message.
        emit_message_parts(
            &header,
            &mut std::iter::once(query.as_ref()),
            &mut std::iter::empty::<&Record>(),
            &mut std::iter::empty::<&Record>(),
            &mut std::iter::empty::<&Record>(),
            edns,
            &MessageSignature::Unsigned,
            &mut BinEncoder::new(&mut socket.send),
        )?;
        let sent = match &socket.socket {
            UdpSocket::Tokio(udp) => udp.send(&socket.send).await?,
            UdpSocket::Proxy(_) => {
                DnsUdpSocket::send_to(&socket.socket, &socket.send, self.server).await?
            }
        };
        if sent != socket.send.len() {
            return Err(ProtoError::from("incomplete UDP query send"));
        }
        loop {
            let (len, peer) = match &socket.socket {
                UdpSocket::Tokio(udp) => {
                    let len = udp.recv(&mut socket.receive[..receive_size]).await?;
                    crate::infra::packet_debug::check_received(
                        self.server,
                        "udp",
                        &socket.receive[..len],
                    );
                    (len, self.server)
                }
                UdpSocket::Proxy(_) => {
                    DnsUdpSocket::recv_from(&socket.socket, &mut socket.receive[..receive_size])
                        .await?
                }
            };
            if peer.ip() != self.server.ip()
                || peer.port() != self.server.port()
                || len < 2
                || u16::from_be_bytes([socket.receive[0], socket.receive[1]]) != id
            {
                continue;
            }
            let response = match Self::decode_response(
                &socket.receive[..len],
                &query,
                original_query.as_ref(),
            ) {
                Ok(Some(response)) => Ok(response),
                Ok(None) => continue,
                Err(error) if matches!(error.kind(), ProtoErrorKind::QueryCaseMismatch) => {
                    return Err(error);
                }
                Err(error) => Err(error),
            };
            let mut idle = self.idle.lock().unwrap();
            if idle.len() < Self::MAX_IDLE_SOCKETS {
                idle.push(socket);
            }
            drop(idle);
            return response;
        }
    }

    // Keep the large protocol parsing values out of the suspended exchange.
    #[inline(never)]
    fn decode_response(
        bytes: &[u8],
        query: &Query,
        original_query: Option<&Query>,
    ) -> Result<Option<DnsResponse>, ProtoError> {
        let Ok(mut response) = HickoryDnsResponse::from_buffer(bytes.to_vec()) else {
            return Ok(None);
        };
        if !response.queries().iter().all(|reply| reply == query) {
            return Ok(None);
        }
        if let Some(original) = original_query {
            if !response
                .queries()
                .iter()
                .all(|reply| query.name().eq_case(reply.name()))
            {
                return Err(ProtoErrorKind::QueryCaseMismatch.into());
            }
            for reply in response.queries_mut() {
                if reply == original {
                    *reply = original.clone();
                }
            }
        }
        ProtoError::from_response(response).map(|response| Some(response.into()))
    }

    async fn bind(&self) -> io::Result<UdpSocket> {
        let local_ip = if self.server.is_ipv4() {
            Ipv4Addr::UNSPECIFIED.into()
        } else {
            Ipv6Addr::UNSPECIFIED.into()
        };
        for _ in 0..10 {
            let port = if self.options.os_port_selection {
                0
            } else {
                rand::random_range(1024..=u16::MAX)
            };
            if self.options.avoid_local_udp_ports.contains(&port) {
                continue;
            }
            match self
                .provider
                .bind_udp(SocketAddr::new(local_ip, port), self.server)
                .await
            {
                Ok(socket) => {
                    if self
                        .options
                        .avoid_local_udp_ports
                        .contains(&socket.local_addr()?.port())
                    {
                        continue;
                    }
                    if let UdpSocket::Tokio(socket) = &socket {
                        socket.connect(self.server).await?;
                    }
                    return Ok(socket);
                }
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
                // Windows also rejects randomly chosen reserved/exclusive ports
                // with WSAEACCES. Retry another port, as for an occupied one.
                Err(error) if cfg!(windows) && port != 0 && error.raw_os_error() == Some(10013) => {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "no available UDP source port",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libdns::proto::{
        op::{Edns, Message, Query, ResponseCode},
        rr::{Record, RecordType},
        xfer::DnsRequestOptions,
    };
    use std::time::Duration;
    use tokio::{net, task::JoinSet};

    fn client(server: SocketAddr, timeout: Duration) -> UdpClient {
        let mut options = ResolverOpts::default();
        options.timeout = timeout;
        UdpClient::new(
            server,
            Arc::new(options),
            TokioRuntimeProvider::new(None, None, None),
        )
    }

    fn request(name: &str) -> Query {
        Query::query(crate::dns::Name::from_ascii(name).unwrap(), RecordType::A)
    }

    fn response(request: &Message, suffix: u8) -> Message {
        let mut response = request.to_response();
        response.add_answer(Record::from_rdata(
            request.queries()[0].name().clone(),
            30,
            Ipv4Addr::new(192, 0, 2, suffix).into(),
        ));
        response
    }

    #[test]
    fn randomized_queries_require_the_echoed_case() {
        let original = request("mixed.example.");
        let randomized = request("MiXeD.ExAmPlE.");
        let mut message = Message::query();
        message.add_query(original.clone());
        let bytes = response(&message, 1).to_vec().unwrap();
        let error = UdpClient::decode_response(&bytes, &randomized, Some(&original)).unwrap_err();
        assert!(matches!(error.kind(), ProtoErrorKind::QueryCaseMismatch));
    }

    #[tokio::test]
    async fn reuses_socket_and_rejects_mismatched_datagrams() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let server = net::UdpSocket::bind(bind).await.unwrap();
            let client = client(server.local_addr().unwrap(), Duration::from_secs(2));
            let worker = tokio::spawn(async move {
                let mut buf = [0; 4096];
                let mut previous_peer = None;
                for i in 1..=3 {
                    let (len, peer) = server.recv_from(&mut buf).await.unwrap();
                    if let Some(previous) = previous_peer {
                        assert_eq!(peer, previous, "completed exchange should reuse the socket");
                    }
                    previous_peer = Some(peer);
                    let query = Message::from_vec(&buf[..len]).unwrap();
                    assert_eq!(query.extensions().as_ref().unwrap().max_payload(), 1232);
                    assert!(query.extensions().as_ref().unwrap().flags().dnssec_ok);
                    let correct = response(&query, i);
                    let wrong_peer = net::UdpSocket::bind(bind).await.unwrap();
                    wrong_peer
                        .send_to(&response(&query, 254).to_vec().unwrap(), peer)
                        .await
                        .unwrap();
                    let mut wrong = correct.clone();
                    let mut header = *wrong.header();
                    header.set_id(header.id().wrapping_add(1));
                    wrong.set_header(header);
                    server
                        .send_to(&wrong.to_vec().unwrap(), peer)
                        .await
                        .unwrap();
                    wrong = correct.clone();
                    wrong.queries_mut()[0] =
                        Query::query("wrong.example.".parse().unwrap(), RecordType::A);
                    server
                        .send_to(&wrong.to_vec().unwrap(), peer)
                        .await
                        .unwrap();
                    server.send_to(&buf[..2], peer).await.unwrap();
                    server
                        .send_to(&correct.to_vec().unwrap(), peer)
                        .await
                        .unwrap();
                }
            });
            for i in 1..=3 {
                let query = request(if i == 1 {
                    "reuse.example"
                } else if i == 2 {
                    "ReUse.Example."
                } else {
                    "reuse.example."
                });
                let original = query.clone();
                let mut options = DnsRequestOptions::default();
                options.case_randomization = i == 2;
                let mut edns = Edns::new();
                edns.set_max_payload(1232).set_dnssec_ok(true);
                let response = client.lookup(&query, options, Some(&edns)).await.unwrap();
                assert!(query.name().eq_case(original.name()));
                assert_eq!(query.name().is_fqdn(), original.name().is_fqdn());
                assert_eq!(
                    response.queries()[0].name().to_ascii(),
                    if i == 2 {
                        "ReUse.Example."
                    } else {
                        "reuse.example."
                    }
                );
                assert_eq!(
                    response.answers()[0].data().ip_addr(),
                    Some(Ipv4Addr::new(192, 0, 2, i).into())
                );
            }
            worker.await.unwrap();
        }
    }

    #[tokio::test]
    async fn simultaneous_queries_keep_their_answers_when_replies_arrive_in_reverse_order() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let server = net::UdpSocket::bind(bind).await.unwrap();
            let client = Arc::new(client(server.local_addr().unwrap(), Duration::from_secs(2)));
            let worker = tokio::spawn(async move {
                let mut buf = [0; 4096];
                let mut pending = Vec::new();
                for _ in 0..32 {
                    let (len, peer) = server.recv_from(&mut buf).await.unwrap();
                    let query = Message::from_vec(&buf[..len]).unwrap();
                    pending.push((query, peer));
                }
                for (query, peer) in pending.into_iter().rev() {
                    let name = query.queries()[0].name().to_ascii();
                    let suffix = name
                        .trim_start_matches('c')
                        .split('.')
                        .next()
                        .unwrap()
                        .parse()
                        .unwrap();
                    server
                        .send_to(&response(&query, suffix).to_vec().unwrap(), peer)
                        .await
                        .unwrap();
                }
            });
            let mut pending = JoinSet::new();
            for i in 1..=32 {
                let client = client.clone();
                pending.spawn(async move {
                    let name = format!("c{i}.example.");
                    let reply = client
                        .lookup(&request(&name), DnsRequestOptions::default(), None)
                        .await
                        .unwrap();
                    assert_eq!(reply.queries()[0].name().to_ascii(), name);
                    assert_eq!(
                        reply.answers()[0].data().ip_addr(),
                        Some(Ipv4Addr::new(192, 0, 2, i).into())
                    );
                });
            }
            while let Some(result) = pending.join_next().await {
                result.unwrap();
            }
            worker.await.unwrap();
        }
    }

    #[tokio::test]
    async fn failed_exchanges_are_discarded_and_negative_answers_keep_their_code() {
        let server = net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client = Arc::new(client(
            server.local_addr().unwrap(),
            Duration::from_millis(50),
        ));
        let error = client
            .lookup(
                &request("timeout.example."),
                DnsRequestOptions::default(),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(error.kind(), ProtoErrorKind::Timeout));
        assert!(client.idle.lock().unwrap().is_empty());
        let mut buf = [0; 4096];
        server.recv_from(&mut buf).await.unwrap();
        let task = {
            let client = client.clone();
            tokio::spawn(async move {
                client
                    .lookup(
                        &request("cancel.example."),
                        DnsRequestOptions::default(),
                        None,
                    )
                    .await
            })
        };
        server.recv_from(&mut buf).await.unwrap();
        task.abort();
        let _ = task.await;
        assert!(client.idle.lock().unwrap().is_empty());
        let worker = tokio::spawn(async move {
            for code in [ResponseCode::NXDomain, ResponseCode::Refused] {
                let (len, peer) = server.recv_from(&mut buf).await.unwrap();
                let mut response = Message::from_vec(&buf[..len]).unwrap().to_response();
                response.set_response_code(code);
                server
                    .send_to(&response.to_vec().unwrap(), peer)
                    .await
                    .unwrap();
            }
        });
        for (name, code) in [
            ("negative.example.", ResponseCode::NXDomain),
            ("refused.example.", ResponseCode::Refused),
        ] {
            let error = client
                .lookup(&request(name), DnsRequestOptions::default(), None)
                .await
                .unwrap_err();
            let ProtoErrorKind::NoRecordsFound(negative) = error.kind() else {
                panic!("{error}")
            };
            assert_eq!(negative.response_code, code);
            assert_eq!(negative.query.name().to_ascii(), name);
        }
        worker.await.unwrap();
    }
}
