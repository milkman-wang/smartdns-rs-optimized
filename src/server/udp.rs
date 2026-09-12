use super::{DnsHandle, reap_tasks, sanitize_src_address};
use crate::{
    dns::SerialMessage,
    libdns::{Protocol, proto::udp::MAX_RECEIVE_BUFFER_SIZE},
    log,
};
use std::sync::Arc;
use tokio::{net, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub fn serve(socket: net::UdpSocket, handler: DnsHandle) -> CancellationToken {
    let token = CancellationToken::new();
    let cancellation_token = token.clone();

    tokio::spawn(async move {
        let socket = Arc::new(socket);
        let mut buffer = [0u8; MAX_RECEIVE_BUFFER_SIZE];
        let mut inner_join_set = JoinSet::new();
        loop {
            let message = tokio::select! {
                message = socket.recv_from(&mut buffer) => message,
                _ = cancellation_token.cancelled() => break,
            };

            let (len, src_addr) = match message {
                Err(e) => {
                    log::warn!("error receiving message on udp_socket: {}", e);
                    continue;
                }
                Ok(message) => message,
            };

            log::debug!("received udp request from: {}", src_addr);

            // verify that the src address is safe for responses
            if let Err(e) = sanitize_src_address(src_addr) {
                log::warn!(
                    "address can not be responded to {src_addr}: {e}",
                    src_addr = src_addr,
                    e = e
                );
                continue;
            }

            let handler = handler.clone();
            let socket = socket.clone();
            let req_message =
                SerialMessage::binary(buffer[..len].to_vec(), src_addr, Protocol::Udp);

            inner_join_set.spawn(async move {
                let res_message = handler.send(req_message).await;
                match Vec::<u8>::try_from(res_message) {
                    Ok(bytes) => {
                        // Reply from the request task without queueing it back
                        // through the receive loop.
                        if let Err(error) = socket.send_to(&bytes, src_addr).await {
                            log::error!("UDP response to {src_addr} failed: {error}");
                        }
                    }
                    Err(error) => log::error!("UDP response encoding failed: {error}"),
                }
            });

            reap_tasks(&mut inner_join_set);
        }
    });
    token
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libdns::proto::{
        op::{Message, Query},
        rr::{Record, RecordType},
    };
    use std::time::Duration;

    #[tokio::test]
    async fn unsupported_packets_do_not_stop_the_listener() {
        use crate::libdns::proto::op::{MessageType, OpCode, ResponseCode};
        let app = crate::app::App::new(Arc::new(crate::dns_conf::RuntimeConfig::default()));
        let socket = net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let token = serve(socket, app.dns_handle());
        let client = net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.connect(address).await.unwrap();
        for (id, (kind, opcode, code)) in [
            (MessageType::Query, OpCode::Status, ResponseCode::NotImp),
            (MessageType::Query, OpCode::Notify, ResponseCode::NotImp),
            (MessageType::Query, OpCode::Update, ResponseCode::NotImp),
            (MessageType::Query, OpCode::Unknown(6), ResponseCode::NotImp),
            (MessageType::Response, OpCode::Query, ResponseCode::FormErr),
            (MessageType::Query, OpCode::Query, ResponseCode::ServFail),
        ]
        .into_iter()
        .enumerate()
        {
            let mut request = Message::new(id as u16, kind, opcode);
            request.add_query(Query::query(
                "listener.example.".parse().unwrap(),
                RecordType::A,
            ));
            client.send(&request.to_vec().unwrap()).await.unwrap();
            let mut bytes = [0; 4096];
            let len = tokio::time::timeout(Duration::from_secs(1), client.recv(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            let response = Message::from_vec(&bytes[..len]).unwrap();
            assert_eq!(response.id(), request.id());
            assert_eq!(response.op_code(), opcode);
            assert_eq!(response.response_code(), code);
            assert_eq!(response.queries(), request.queries());
        }
        token.cancel();
    }

    #[tokio::test]
    async fn concurrent_replies_preserve_client_address_and_question() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let socket = net::UdpSocket::bind(bind).await.unwrap();
            let addr = socket.local_addr().unwrap();
            let (mut incoming, handle) = DnsHandle::mock();
            let token = serve(socket, handle);
            let processor = tokio::spawn(async move {
                let mut replies = JoinSet::new();
                for _ in 0..32 {
                    let (message, _, sender) = incoming.recv().await.unwrap();
                    let peer = message.addr();
                    assert_eq!(message.protocol(), Protocol::Udp);
                    let request = Message::try_from(message).unwrap();
                    replies.spawn(async move {
                        tokio::time::sleep(Duration::from_millis((request.id() % 4) as u64)).await;
                        let mut response = request.to_response();
                        response.add_answer(Record::from_rdata(
                            request.queries()[0].name().clone(),
                            30,
                            "192.0.2.1".parse::<std::net::IpAddr>().unwrap().into(),
                        ));
                        sender
                            .send(SerialMessage::raw(response, peer, Protocol::Udp))
                            .ok();
                    });
                }
                while replies.join_next().await.is_some() {}
            });
            let mut clients = JoinSet::new();
            for id in 0..32 {
                clients.spawn(async move {
                    let client = net::UdpSocket::bind(bind).await.unwrap();
                    client.connect(addr).await.unwrap();
                    let name = format!("client-{id}.example.").parse().unwrap();
                    let mut request = Message::query();
                    let mut header = request.header().to_owned();
                    header.set_id(id);
                    request.set_header(header);
                    request.add_query(Query::query(name, RecordType::A));
                    client.send(&request.to_vec().unwrap()).await.unwrap();
                    let mut bytes = [0; 4096];
                    let len = tokio::time::timeout(Duration::from_secs(2), client.recv(&mut bytes))
                        .await
                        .unwrap()
                        .unwrap();
                    let response = Message::from_vec(&bytes[..len]).unwrap();
                    assert_eq!(response.id(), id);
                    assert_eq!(response.queries(), request.queries());
                    assert_eq!(
                        response.answers()[0].data().ip_addr(),
                        Some("192.0.2.1".parse().unwrap())
                    );
                });
            }
            while let Some(result) = clients.join_next().await {
                result.unwrap();
            }
            processor.await.unwrap();
            token.cancel();
        }
    }
}
