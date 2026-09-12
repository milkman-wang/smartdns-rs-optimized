use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Query;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, FromRequest, Request, State},
};
use serde::{Deserialize, Serialize};

use super::openapi::{
    IntoParams, IntoRouter, ToSchema,
    http::{get, post},
    routes,
};
use super::{ServeState, StatefulRouter};
use crate::{dns::SerialMessage, libdns::Protocol, log};

pub fn routes() -> StatefulRouter {
    routes![serve_dns_get, serve_dns].into_router()
}

#[get("/dns-query", tag="DNS", params(QueryParam), responses(
    (status = 200, description = "DNS response", body = DnsResponse)
))]
async fn serve_dns_get(
    State(state): State<Arc<ServeState>>,
    Query(parameters): Query<QueryParam>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    // https://developers.cloudflare.com/1.1.1.1/encryption/dns-over-https/make-api-requests/dns-json/
    match process(&state.dns_handle, req, addr, Some(parameters)).await {
        Ok((content_type, bytes)) => {
            let mut res = Body::from(bytes).into_response();
            res.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            res
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(r#"{{ "error": "{err}" }}"#),
        )
            .into_response(),
    }
}

#[post("/dns-query", tag = "DNS")]
async fn serve_dns(
    State(state): State<Arc<ServeState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    match process(&state.dns_handle, req, addr, None).await {
        Ok((content_type, bytes)) => {
            let mut res = Body::from(bytes).into_response();
            res.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            res
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(r#"{{ "error": "{err}" }}"#),
        )
            .into_response(),
    }
}

async fn process(
    dns_handle: &crate::server::DnsHandle,
    req: Request,
    addr: SocketAddr,
    query_param: Option<QueryParam>,
) -> anyhow::Result<(&'static str, Bytes)> {
    const APPLICATION_DNS_MESSAGE: &str = "application/dns-message";
    const APPLICATION_JSON: &str = "application/json";

    let accept = match req.headers().get(header::ACCEPT).map(|s| s.to_str()) {
        Some(Ok(s)) => s,
        _ => "",
    };

    log::debug!(
        "DoH {} {} {}",
        req.method().as_str(),
        req.uri().to_string(),
        accept
    );

    let wire_query = query_param
        .as_ref()
        .is_some_and(|param| param.dns.is_some());
    let accept_dns_message = accept == APPLICATION_DNS_MESSAGE
        || wire_query
        || (query_param.is_none() && accept != APPLICATION_JSON);

    let req_msg = match query_param {
        Some(QueryParam {
            dns: Some(encoded), ..
        }) => {
            use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
            let bytes = URL_SAFE_NO_PAD.decode(encoded)?;
            SerialMessage::binary(bytes, addr, Protocol::Https)
        }
        Some(query_param) => {
            // https://developers.cloudflare.com/1.1.1.1/encryption/dns-over-https/make-api-requests/dns-json/
            use crate::libdns::proto::{
                op::{Edns, Message, Query},
                rr::{Name, RecordType},
            };

            let name: Name = query_param
                .name
                .ok_or_else(|| anyhow::anyhow!("missing DNS query: supply dns or name"))?
                .parse()?;
            let query_type: RecordType = query_param.query_type.parse().unwrap_or(RecordType::A);

            let dnssec = query_param.dnssec;
            let checking_disabled = query_param.checking_disabled;

            let mut message = Message::query();
            message.add_query(Query::query(name, query_type));
            message.set_checking_disabled(checking_disabled);
            if dnssec {
                let mut edns = Edns::new();
                edns.set_dnssec_ok(dnssec);
                message.set_edns(edns);
            }

            SerialMessage::raw(message, addr, Protocol::Https)
        }
        _ => {
            let bytes = Bytes::from_request(req, &()).await?;
            SerialMessage::binary(bytes.into(), addr, Protocol::Https)
        }
    };

    let res_msg = dns_handle.send(req_msg).await;

    let (content_type, bytes) = if accept_dns_message {
        (APPLICATION_DNS_MESSAGE, res_msg.try_into()?)
    } else {
        let message = crate::libdns::proto::op::Message::try_from(res_msg)?;
        (
            APPLICATION_JSON,
            serde_json::to_vec(&DnsResponse::from(&message))?,
        )
    };

    Ok((content_type, bytes.into()))
}

#[derive(Deserialize, IntoParams)]
struct QueryParam {
    /// Base64url encoded DNS wire query (RFC 8484).
    dns: Option<String>,

    /// Query name
    name: Option<String>,

    /// Query type (either a numeric value or text ↗).
    #[serde(default = "QueryParam::default_query_type", rename = "type")]
    query_type: String,

    /// DO bit - whether the client wants DNSSEC data (either empty or one of 0, false, 1, or true).
    #[serde(default, rename = "do")]
    dnssec: bool,

    /// CD bit - disable validation (either empty or one of 0, false, 1, or true).
    #[serde(default, rename = "cd")]
    checking_disabled: bool,
}

impl QueryParam {
    fn default_query_type() -> String {
        "A".to_string()
    }
}

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
struct DnsResponse {
    /// The Response Code of the DNS Query
    status: u16,

    /// If true, it means the truncated bit was set.
    /// This happens when the DNS answer is larger
    /// than a single UDP or TCP packet. TC will
    /// almost always be false with Cloudflare
    /// DNS over HTTPS because Cloudflare supports
    /// the maximum response size.
    TC: bool,

    /// If true, it means the Recursive Desired
    /// bit was set. This is always set to true
    /// for Cloudflare DNS over HTTPS.
    RD: bool,

    /// If true, it means the Recursion Available
    /// bit was set. This is always set to true
    /// for Cloudflare DNS over HTTPS.
    RA: bool,

    /// If true, it means that every record
    /// in the answer was verified with DNSSEC.
    AD: bool,

    /// If true, the client asked to disable
    /// DNSSEC validation. In this case,
    /// Cloudflare will still fetch the DNSSEC-related records,
    /// but it will not attempt to validate the records.
    CD: bool,

    Question: Vec<Question>,
    Answer: Vec<Answer>,
}

#[derive(Serialize, ToSchema)]
struct Question {
    name: String,
    r#type: u16,
}

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
struct Answer {
    name: String,
    r#type: u16,
    TTL: u32,
    data: String,
}

impl From<&crate::libdns::proto::op::Message> for DnsResponse {
    fn from(message: &crate::libdns::proto::op::Message) -> Self {
        DnsResponse {
            status: message.response_code().into(),
            TC: message.truncated(),
            RD: message.recursion_desired(),
            RA: message.recursion_available(),
            AD: message.authentic_data(),
            CD: message.checking_disabled(),
            Question: message
                .queries()
                .iter()
                .map(|q| Question {
                    name: q.name().to_string(),
                    r#type: q.query_type().into(),
                })
                .collect(),
            Answer: message
                .answers()
                .iter()
                .map(|r| Answer {
                    name: r.name().to_string(),
                    r#type: r.record_type().into(),
                    TTL: r.ttl(),
                    data: r.data().to_string(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dns::DnsRequest,
        libdns::proto::{
            op::{Message, Query},
            rr::{RData, Record},
        },
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    #[tokio::test]
    async fn test_doh_get_post_wire_and_json_queries() {
        let mut request = Message::query();
        request.add_query(Query::query(
            "resolver.example.".parse().unwrap(),
            crate::dns::RecordType::A,
        ));
        let wire = request.to_vec().unwrap();
        for mode in ["get", "post", "json"] {
            let (mut incoming, dns_handle) = crate::server::DnsHandle::mock();
            let worker = tokio::spawn(async move {
                let (packet, _, reply) = incoming.recv().await.unwrap();
                let request = DnsRequest::try_from(packet).unwrap();
                assert_eq!(request.query().name().to_ascii(), "resolver.example.");
                let mut response = request.to_response();
                response
                    .set_authentic_data(true)
                    .set_checking_disabled(false);
                response.add_answer(Record::from_rdata(
                    request.query().name().clone().into(),
                    30,
                    RData::A("192.0.2.9".parse().unwrap()),
                ));
                assert!(reply.send(response.into()).is_ok());
            });
            let params = match mode {
                "get" => Some(
                    serde_json::from_value(
                        serde_json::json!({"dns": URL_SAFE_NO_PAD.encode(&wire)}),
                    )
                    .unwrap(),
                ),
                "json" => Some(
                    serde_json::from_value(serde_json::json!({"name": "resolver.example."}))
                        .unwrap(),
                ),
                _ => None,
            };
            let request = Request::builder()
                .method(if mode == "post" { "POST" } else { "GET" })
                .body(Body::from(if mode == "post" {
                    wire.clone()
                } else {
                    vec![]
                }))
                .unwrap();
            let (content_type, body) = process(
                &dns_handle,
                request,
                "127.0.0.1:12345".parse().unwrap(),
                params,
            )
            .await
            .unwrap();
            if mode == "json" {
                assert_eq!(content_type, "application/json");
                let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(value["Answer"][0]["data"], "192.0.2.9");
                assert_eq!(value["AD"], true);
                assert_eq!(value["CD"], false);
            } else {
                assert_eq!(content_type, "application/dns-message");
                let response = Message::from_vec(&body).unwrap();
                assert_eq!(
                    response.answers()[0].data(),
                    &RData::A("192.0.2.9".parse().unwrap())
                );
            }
            worker.await.unwrap();
        }
    }
}
