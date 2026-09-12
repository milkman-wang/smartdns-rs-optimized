#![allow(unused_imports)]

use std::borrow::Borrow;
use std::fmt::Debug;

use std::net::IpAddr;
use std::{str::FromStr, sync::Arc, time::Duration};

use crate::dns_error::LookupError;
use crate::dns_rule::DomainRuleTreeNode;

use crate::config::ServerOpts;
use crate::dns_conf::RuntimeConfig;

pub use crate::dns_rule::DomainRuleGetter;

pub use crate::libdns::proto::{
    ProtoErrorKind, op,
    rr::{self, Name, RData, Record, RecordType, rdata::SOA},
};

pub use crate::libdns::{
    proto::xfer::Protocol,
    resolver::{config::NameServerConfig, lookup::Lookup},
};

#[derive(Clone)]
pub struct DnsContext {
    cfg: Arc<RuntimeConfig>,
    pub server_opts: ServerOpts,
    pub domain_rule: Option<Arc<DomainRuleTreeNode>>,
    pub fastest_speed: Duration,
    pub source: LookupFrom,
    pub no_cache: bool,
    pub is_dualstack: bool,
}

impl DnsContext {
    pub fn new(name: &Name, cfg: Arc<RuntimeConfig>, server_opts: ServerOpts) -> Self {
        let group_name = server_opts.rule_group.as_deref().unwrap_or_default();
        let domain_rule = cfg.find_domain_rule(name, group_name);

        let no_cache = domain_rule.get(|n| n.no_cache).unwrap_or_default();

        DnsContext {
            cfg,
            server_opts,
            domain_rule,
            fastest_speed: Default::default(),
            source: Default::default(),
            no_cache,
            is_dualstack: false,
        }
    }

    #[inline]
    pub fn cfg(&self) -> &Arc<RuntimeConfig> {
        &self.cfg
    }

    #[inline]
    pub fn server_opts(&self) -> &ServerOpts {
        &self.server_opts
    }

    pub fn server_group_name(&self) -> &str {
        match self.server_opts().group() {
            Some(n) => n,
            None => {
                let mut node = self.domain_rule.as_ref();

                while let Some(rule) = node {
                    if let Some(name) = rule.nameserver.as_deref() {
                        return name;
                    }

                    node = rule.zone();
                }

                "default"
            }
        }
    }
}

#[derive(Clone)]
pub enum LookupFrom {
    None,
    Cache,
    Static,
    Zone(String),
    Server(String),
}

impl Debug for LookupFrom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Cache => write!(f, "Cache"),
            Self::Static => write!(f, "Static"),
            Self::Zone(arg0) => write!(f, "Zone: {arg0}"),
            Self::Server(arg0) => write!(f, "Server: {arg0}"),
        }
    }
}

impl Default for LookupFrom {
    #[inline]
    fn default() -> Self {
        Self::None
    }
}

mod serial_message {

    use crate::dns_error::LookupError;
    use crate::libdns::Protocol;
    use crate::libdns::proto::{
        ProtoError,
        op::{Header, Query, ResponseCode},
        serialize::binary::{BinEncodable, BinEncoder},
    };
    use crate::{config::ServerOpts, libdns::proto::op::Message};
    use bytes::Bytes;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::sync::Arc;

    use super::{DnsRequest, DnsResponse};

    pub enum SerialMessage {
        Raw(Box<Message>, SocketAddr, Protocol),
        Bytes(Vec<u8>, SocketAddr, Protocol),
        Reply(
            Arc<Message>,
            Header,
            Option<Arc<[u8]>>,
            SocketAddr,
            Protocol,
        ),
    }

    impl SerialMessage {
        pub fn binary(bytes: Vec<u8>, addr: SocketAddr, protocol: Protocol) -> Self {
            Self::Bytes(bytes, addr, protocol)
        }
        pub fn raw(message: Message, addr: SocketAddr, protocol: Protocol) -> Self {
            Self::Raw(message.into(), addr, protocol)
        }

        /// Use the request's reply header without copying shared cached records.
        pub fn reply(
            mut message: DnsResponse,
            mut header: Header,
            addr: SocketAddr,
            protocol: Protocol,
        ) -> Self {
            header.set_truncated(header.truncated() || message.truncated());
            if header.response_code() == ResponseCode::NoError {
                header.set_response_code(message.response_code());
            } else if header.response_code() != message.response_code() {
                message.set_response_code(header.response_code());
            }
            let (message, wire) = message.into_reply_parts();
            Self::Reply(message, header, wire, addr, protocol)
        }

        pub fn is_binray(&self) -> bool {
            matches!(self, SerialMessage::Bytes(_, _, _))
        }

        pub fn protocol(&self) -> Protocol {
            match self {
                SerialMessage::Raw(_, _, p) => *p,
                SerialMessage::Bytes(_, _, p) => *p,
                SerialMessage::Reply(_, _, _, _, p) => *p,
            }
        }

        pub fn addr(&self) -> SocketAddr {
            match self {
                SerialMessage::Raw(_, a, _) => *a,
                SerialMessage::Bytes(_, a, _) => *a,
                SerialMessage::Reply(_, _, _, a, _) => *a,
            }
        }
    }

    impl From<Query> for SerialMessage {
        fn from(query: Query) -> Self {
            let mut message = Message::query();
            message.add_query(query);
            message.into()
        }
    }

    impl From<Message> for SerialMessage {
        fn from(message: Message) -> Self {
            Self::raw(
                message,
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 0)),
                Protocol::Udp,
            )
        }
    }

    impl TryFrom<SerialMessage> for crate::libdns::proto::xfer::SerialMessage {
        type Error = ProtoError;
        fn try_from(value: SerialMessage) -> Result<Self, Self::Error> {
            Ok(match value {
                SerialMessage::Bytes(bytes, addr, _) => Self::new(bytes, addr),
                SerialMessage::Raw(message, addr, _) => Self::new(message.to_vec()?, addr),
                SerialMessage::Reply(message, mut header, wire, addr, _) => {
                    let mut bytes = match wire {
                        Some(wire) => wire.to_vec(),
                        None => message.to_vec()?,
                    };
                    // Encoding computes section counts from the records. Keep
                    // those counts when replacing the request-dependent flags.
                    let counts: [u8; 8] = bytes[4..12].try_into().unwrap();
                    // The encoder can also truncate records at the wire limit.
                    header.set_truncated(header.truncated() || bytes[2] & 0x02 != 0);
                    header.emit(&mut BinEncoder::new(&mut bytes))?;
                    bytes[4..12].copy_from_slice(&counts);
                    Self::new(bytes, addr)
                }
            })
        }
    }

    impl TryFrom<SerialMessage> for Vec<u8> {
        type Error = ProtoError;
        #[inline]
        fn try_from(value: SerialMessage) -> Result<Self, Self::Error> {
            Ok(crate::libdns::proto::xfer::SerialMessage::try_from(value)?
                .into_parts()
                .0)
        }
    }

    impl TryFrom<SerialMessage> for Bytes {
        type Error = ProtoError;
        #[inline]
        fn try_from(value: SerialMessage) -> Result<Self, Self::Error> {
            Ok(crate::libdns::proto::xfer::SerialMessage::try_from(value)?
                .into_parts()
                .0
                .into())
        }
    }

    impl TryFrom<SerialMessage> for Message {
        type Error = ProtoError;

        fn try_from(value: SerialMessage) -> Result<Self, Self::Error> {
            match value {
                SerialMessage::Raw(message, _, _) => Ok(*message),
                SerialMessage::Bytes(bytes, _, _) => Message::from_vec(&bytes),
                SerialMessage::Reply(message, header, _, _, _) => {
                    let mut message = Arc::unwrap_or_clone(message);
                    message.set_header(header);
                    Ok(message)
                }
            }
        }
    }
}

mod request {

    use std::{fmt::Debug, net::SocketAddr, ops::Deref, sync::Arc};

    use crate::libdns::{
        Protocol,
        proto::{
            ProtoError,
            op::{LowerQuery, Message, Query},
            rr::{Name, RecordType},
        },
    };

    use super::{DnsError, SerialMessage};

    #[derive(Clone)]
    pub struct DnsRequest {
        id: u16,
        /// Message with the associated query or update data
        query: LowerQuery,
        message: Arc<Message>,
        /// Source address of the Client
        src: SocketAddr,
        /// Protocol of the request
        protocol: Protocol,
    }

    impl DnsRequest {
        pub fn new(message: Message, src_addr: SocketAddr, protocol: Protocol) -> Self {
            let id = message.id();
            let query = message.queries().first().cloned().unwrap_or_default();
            Self {
                id,
                query: query.into(),
                message: Arc::new(message),
                src: src_addr,
                protocol,
            }
        }

        /// see `Header::id()`
        pub fn id(&self) -> u16 {
            self.id
        }

        /// ```text
        /// Question        Carries the query name and other query parameters.
        /// ```
        #[inline]
        pub fn query(&self) -> &LowerQuery {
            &self.query
        }

        /// The IP address from which the request originated.
        #[inline]
        pub fn src(&self) -> SocketAddr {
            self.src
        }

        /// The protocol that was used for the request
        #[inline]
        pub fn protocol(&self) -> Protocol {
            self.protocol
        }

        pub fn with_cname(&self, name: Name) -> Self {
            Self {
                id: self.id,
                query: LowerQuery::from(Query::query(name, self.query().query_type())),
                message: self.message.clone(),
                src: self.src,
                protocol: self.protocol,
            }
        }

        pub fn set_query_type(&mut self, query_type: RecordType) {
            let mut query = self.query.original().clone();
            query.set_query_type(query_type);
            self.query = LowerQuery::from(query)
        }

        pub fn is_dnssec(&self) -> bool {
            let rtype = self.query().query_type();
            self.extensions()
                .as_ref()
                .map(|e| e.flags().dnssec_ok)
                .unwrap_or(rtype.is_dnssec())
        }
    }

    impl Debug for DnsRequest {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let id = self.id();
            let src_addr = self.src();
            let protocol = self.protocol();
            let query = self.query();
            let query_name = query.name();
            let query_type = query.query_type();
            let query_class = query.query_class();

            let message_type = self.message_type();
            let is_dnssec = self.is_dnssec();
            let qop_code = self.op_code();
            let qflags = self.flags();

            write!(
                f,
                "{id} src:{proto}://{addr}#{port} type:{message_type} dnssec:{is_dnssec} {op}:{query}:{qtype}:{class} qflags:{qflags}",
                id = id,
                proto = protocol,
                addr = src_addr.ip(),
                port = src_addr.port(),
                message_type = message_type,
                is_dnssec = is_dnssec,
                op = qop_code,
                query = query_name,
                qtype = query_type,
                class = query_class,
                qflags = qflags,
            )
        }
    }

    impl std::ops::Deref for DnsRequest {
        type Target = Message;

        fn deref(&self) -> &Self::Target {
            self.message.as_ref()
        }
    }

    impl From<Query> for DnsRequest {
        fn from(query: Query) -> Self {
            use std::net::{Ipv4Addr, SocketAddrV4};

            let mut message = Message::query();
            message.add_query(query.clone());

            Self {
                id: message.id(),
                query: query.into(),
                message: Arc::new(message),
                src: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 53)),
                protocol: Protocol::Udp,
            }
        }
    }

    impl TryFrom<SerialMessage> for DnsRequest {
        type Error = ProtoError;

        fn try_from(value: SerialMessage) -> Result<Self, Self::Error> {
            match value {
                SerialMessage::Reply(message, header, _, src_addr, protocol) => {
                    let mut message = Arc::unwrap_or_clone(message);
                    message.set_header(header);
                    Ok(DnsRequest::new(message, src_addr, protocol))
                }
                SerialMessage::Raw(message, src_addr, protocol) => {
                    Ok(DnsRequest::new(*message, src_addr, protocol))
                }
                SerialMessage::Bytes(bytes, src_addr, protocol) => {
                    use crate::libdns::proto::serialize::binary::{BinDecodable, BinDecoder};
                    let mut decoder = BinDecoder::new(&bytes);
                    let message = Message::read(&mut decoder).inspect_err(|_| {
                        crate::infra::packet_debug::save(
                            "server",
                            src_addr,
                            &protocol.to_string(),
                            &bytes,
                        );
                    })?;
                    Ok(DnsRequest::new(message, src_addr, protocol))
                }
            }
        }
    }
}

mod response {

    use crate::dns_client::MAX_TTL;
    use crate::libdns::proto::{
        ProtoError,
        op::{self, Header, Message, MessageType, Query},
        rr::{RData, Record},
    };
    use crate::libdns::resolver::TtlClip as _;

    use std::net::IpAddr;
    use std::ops::Deref;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::DnsRequest;

    static DEFAULT_QUERY: once_cell::sync::Lazy<Query> = once_cell::sync::Lazy::new(Query::default);

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub enum ProbeResult {
        #[default]
        Unchecked,
        Failed,
        Measured(Duration),
    }

    #[derive(Debug, Clone, Eq)]
    pub struct DnsResponse {
        message: Arc<Message>,
        prepared_wire: Option<Arc<[u8]>>,
        wire_len: Option<std::num::NonZeroUsize>,
        valid_until: Instant,
        name_server_group: Option<String>,
        probe_result: ProbeResult,
    }

    impl PartialEq for DnsResponse {
        fn eq(&self, other: &Self) -> bool {
            self.message == other.message && self.name_server_group == other.name_server_group
        }
    }

    impl DnsResponse {
        pub fn new_with_max_ttl<R, I>(query: Query, records: R) -> Self
        where
            R: IntoIterator<Item = Record, IntoIter = I>,
            I: Iterator<Item = Record>,
        {
            let valid_until = Instant::now() + Duration::from_secs(u64::from(MAX_TTL));
            Self::new_with_deadline(query, records, valid_until)
        }

        pub fn new_with_deadline<R, I>(query: Query, records: R, valid_until: Instant) -> Self
        where
            R: IntoIterator<Item = Record, IntoIter = I>,
            I: Iterator<Item = Record>,
        {
            use op::message::{HeaderCounts, update_header_counts};
            let mut message = Message::query().to_response();
            message.add_query(query);
            message.add_answers(records);

            let header = update_header_counts(
                message.header(),
                message.truncated(),
                HeaderCounts {
                    query_count: message.queries().len(),
                    answer_count: message.answers().len(),
                    authority_count: message.authorities().len(),
                    additional_count: message.additionals().len(),
                },
            );

            message.set_header(header);

            Self {
                message: Arc::new(message),
                prepared_wire: None,
                wire_len: None,
                valid_until,
                name_server_group: None,
                probe_result: ProbeResult::Unchecked,
            }
        }

        pub fn empty() -> Self {
            Self {
                message: Arc::new(Message::query()),
                prepared_wire: None,
                wire_len: None,
                valid_until: Instant::now(),
                name_server_group: None,
                probe_result: ProbeResult::Unchecked,
            }
        }

        /// Return new instance with given rdata and the maximum TTL.
        pub fn from_rdata(query: Query, rdata: RData) -> Self {
            let record = Record::from_rdata(query.name().clone(), MAX_TTL, rdata);
            Self::new_with_max_ttl(query, vec![record])
        }

        pub fn query(&self) -> &Query {
            self.deref().queries().first().unwrap_or(&DEFAULT_QUERY)
        }

        pub fn message(&self) -> &Message {
            &self.message
        }

        /// Received payload size for cache accounting while the message
        /// structure is unchanged. TTL and routing metadata do not change it.
        pub fn wire_len(&self) -> Option<usize> {
            self.wire_len.map(std::num::NonZeroUsize::get)
        }

        /// Retain the encoding of an admitted hot-cache response.
        pub fn prepare_wire(&mut self) {
            if self.prepared_wire.is_none() {
                self.prepared_wire = self.message.to_vec().ok().map(Arc::from);
            }
        }

        /// Estimated retained encoding storage, including its Arc counters.
        pub fn prepared_wire_memory(&self) -> usize {
            self.prepared_wire
                .as_ref()
                .map_or(0, |wire| wire.len() + 2 * std::mem::size_of::<usize>())
        }

        /// Encode the current message, reusing an unchanged prepared response.
        pub fn to_vec(&self) -> Result<Vec<u8>, ProtoError> {
            match &self.prepared_wire {
                Some(wire) => Ok(wire.to_vec()),
                None => self.message.to_vec(),
            }
        }

        pub fn valid_until(&self) -> Instant {
            self.valid_until
        }

        pub fn with_valid_until(mut self, valid_until: Instant) -> Self {
            self.valid_until = valid_until;
            self
        }

        pub fn name_server_group(&self) -> Option<&str> {
            self.name_server_group.as_deref()
        }

        pub fn with_name_server_group(mut self, group_name: String) -> Self {
            self.name_server_group = Some(group_name);
            self
        }

        pub fn records(&self) -> &[Record] {
            self.answers()
        }

        pub fn probe_result(&self) -> ProbeResult {
            self.probe_result
        }

        pub fn with_probe_result(mut self, result: ProbeResult) -> Self {
            self.probe_result = result;
            self
        }

        pub fn record_iter(&self) -> std::slice::Iter<'_, Record> {
            self.answers().iter()
        }

        pub fn ip_addrs(&self) -> Vec<IpAddr> {
            self.ip_addrs_iter().collect()
        }

        pub fn ip_addrs_iter(&self) -> impl Iterator<Item = IpAddr> + '_ {
            self.message()
                .answers()
                .iter()
                .flat_map(|r| r.data().ip_addr())
        }

        pub fn set_valid_until_max(&mut self) {
            self.set_valid_until(MAX_TTL)
        }

        pub fn set_valid_until(&mut self, ttl: u32) {
            let valid_until = Instant::now() + Duration::from_secs(ttl as u64);
            self.valid_until = valid_until
        }

        /// Transfer the shared message and encoding without copying their contents.
        pub(super) fn into_reply_parts(self) -> (Arc<Message>, Option<Arc<[u8]>>) {
            (self.message, self.prepared_wire)
        }
    }

    impl std::ops::Deref for DnsResponse {
        type Target = Message;

        fn deref(&self) -> &Self::Target {
            &self.message
        }
    }

    impl std::ops::DerefMut for DnsResponse {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.wire_len = None;
            self.prepared_wire = None;
            Arc::make_mut(&mut self.message)
        }
    }

    impl From<Message> for DnsResponse {
        fn from(message: Message) -> Self {
            let valid_until = Instant::now()
                + Duration::from_secs(
                    message
                        .answers()
                        .iter()
                        .map(|r| r.ttl())
                        .min()
                        .unwrap_or(MAX_TTL) as u64,
                );
            Self {
                message: Arc::new(message),
                prepared_wire: None,
                wire_len: None,
                valid_until,
                name_server_group: None,
                probe_result: ProbeResult::Unchecked,
            }
        }
    }

    impl From<crate::libdns::proto::xfer::DnsResponse> for DnsResponse {
        fn from(response: crate::libdns::proto::xfer::DnsResponse) -> Self {
            let wire_len = response.as_buffer().len();
            let mut response = Self::from(response.into_message());
            response.wire_len = std::num::NonZeroUsize::new(wire_len);
            response
        }
    }

    impl DnsResponse {
        pub fn max_ttl(&self) -> Option<u32> {
            self.answers().iter().map(|record| record.ttl()).max()
        }

        pub fn min_ttl(&self) -> Option<u32> {
            self.answers().iter().map(|record| record.ttl()).min()
        }

        pub fn set_new_ttl(&mut self, ttl: u32) {
            if self.answers().iter().any(|record| record.ttl() != ttl) {
                self.prepared_wire = None;
                for record in Arc::make_mut(&mut self.message).answers_mut() {
                    record.set_ttl(ttl);
                }
            }
        }

        pub fn set_max_ttl(&mut self, ttl: u32) {
            if !self
                .answers()
                .iter()
                .chain(self.authorities())
                .chain(self.additionals())
                .any(|record| record.ttl() > ttl)
            {
                return;
            }
            self.prepared_wire = None;
            let message = Arc::make_mut(&mut self.message);
            for record in message.answers_mut() {
                record.set_max_ttl(ttl);
            }
            for record in message.authorities_mut() {
                record.set_max_ttl(ttl);
            }
            for record in message.additionals_mut() {
                record.set_max_ttl(ttl);
            }
        }

        pub fn set_min_ttl(&mut self, ttl: u32) {
            if self.answers().iter().any(|record| record.ttl() < ttl) {
                self.prepared_wire = None;
                for record in Arc::make_mut(&mut self.message).answers_mut() {
                    record.set_min_ttl(ttl);
                }
            }
        }
    }
}

pub type DnsRequest = request::DnsRequest;
pub type DnsResponse = response::DnsResponse;
pub use response::ProbeResult;
pub type DnsError = LookupError;
use ipnet::IpAdd;
pub use serial_message::SerialMessage;

#[derive(Debug, Clone, Copy, Default)]
pub enum LookupResponseStrategy {
    #[default]
    FirstPing, // query + ping
    FastestIp,       // ping
    FastestResponse, // query
}

pub trait DefaultSOA {
    fn default_soa() -> Self;
}

impl DefaultSOA for SOA {
    #[inline]
    fn default_soa() -> Self {
        Self::new(
            Name::from_str("a.gtld-servers.net").unwrap(),
            Name::from_str("nstld.verisign-grs.com").unwrap(),
            1800,
            1800,
            900,
            604800,
            86400,
        )
    }
}

impl DefaultSOA for RData {
    fn default_soa() -> Self {
        Self::SOA(SOA::default_soa())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_encoding_is_invalidated_by_ttl_records_and_edns_changes() {
        let mut original = DnsResponse::from_rdata(
            op::Query::query("prepared.example.".parse().unwrap(), RecordType::A),
            "192.0.2.1".parse::<IpAddr>().unwrap().into(),
        );
        original.set_new_ttl(30);
        original.prepare_wire();
        let wire = original.to_vec().unwrap();
        assert!(original.prepared_wire_memory() > wire.len());
        for operation in 0..3 {
            let mut changed = original.clone();
            match operation {
                0 => changed.set_new_ttl(10),
                1 => changed.set_max_ttl(10),
                _ => changed.set_min_ttl(40),
            }
            assert_eq!(changed.prepared_wire_memory(), 0);
            let decoded = op::Message::from_vec(&changed.to_vec().unwrap()).unwrap();
            assert_eq!(
                decoded.answers()[0].ttl(),
                if operation == 2 { 40 } else { 10 }
            );
        }
        let mut changed = original.clone();
        changed.answers_mut()[0].set_data("192.0.2.2".parse::<IpAddr>().unwrap().into());
        let mut edns = op::Edns::new();
        edns.set_max_payload(1232).set_dnssec_ok(true);
        *changed.extensions_mut() = Some(edns);
        let decoded = op::Message::from_vec(&changed.to_vec().unwrap()).unwrap();
        assert_eq!(decoded.answers(), changed.answers());
        assert_eq!(decoded.extensions(), changed.extensions());
        assert_eq!(original.to_vec().unwrap(), wire);
    }

    #[test]
    fn received_payload_size_survives_ttl_changes_and_invalidates_on_record_changes() {
        let name: Name = "wire-size.example.".parse().unwrap();
        let response = DnsResponse::from_rdata(
            op::Query::query(name.clone(), RecordType::A),
            "192.0.2.1".parse::<std::net::IpAddr>().unwrap().into(),
        );
        let wire = response.to_vec().unwrap();
        let received = crate::libdns::proto::xfer::DnsResponse::from_buffer(wire.clone()).unwrap();
        let mut response = DnsResponse::from(received).with_name_server_group("default".into());
        assert_eq!(response.wire_len(), Some(wire.len()));
        response.set_new_ttl(30);
        response.set_max_ttl(20);
        response.set_min_ttl(25);
        assert_eq!(response.answers()[0].ttl(), 25);
        assert_eq!(response.wire_len(), Some(response.to_vec().unwrap().len()));
        response.add_answer(Record::from_rdata(
            name,
            25,
            "192.0.2.2".parse::<std::net::IpAddr>().unwrap().into(),
        ));
        assert_eq!(response.wire_len(), None);
        assert!(response.to_vec().unwrap().len() > wire.len());
    }
    #[test]
    fn shared_responses_keep_independent_record_and_ttl_changes() {
        let original = DnsResponse::from_rdata(
            op::Query::query("shared.example.".parse().unwrap(), RecordType::A),
            "192.0.2.1".parse::<IpAddr>().unwrap().into(),
        );
        let original_ttl = original.answers()[0].ttl();
        let mut modified = original.clone();
        modified.set_max_ttl(5);
        modified.answers_mut()[0].set_data("192.0.2.2".parse::<IpAddr>().unwrap().into());
        assert_eq!(original.answers()[0].ttl(), original_ttl);
        assert_eq!(
            original.ip_addrs(),
            vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(modified.answers()[0].ttl(), 5);
        assert_eq!(
            modified.ip_addrs(),
            vec!["192.0.2.2".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn shared_reply_encoding_preserves_counts_flags_and_extended_response_codes() {
        use op::{Edns, Header, Message, MessageType, OpCode, Query, ResponseCode};
        let mut message = Message::response(7, OpCode::Query);
        message.add_query(Query::query(
            "MiXeD.example.".parse().unwrap(),
            RecordType::A,
        ));
        message.add_answer(Record::from_rdata(
            "MiXeD.example.".parse().unwrap(),
            30,
            "192.0.2.1".parse::<IpAddr>().unwrap().into(),
        ));
        message.add_authority(Record::from_rdata(
            "example.".parse().unwrap(),
            60,
            RData::SOA(SOA::new(
                "ns.example.".parse().unwrap(),
                "hostmaster.example.".parse().unwrap(),
                1,
                60,
                60,
                3600,
                30,
            )),
        ));
        message.add_additional(Record::from_rdata(
            "additional.example.".parse().unwrap(),
            90,
            RData::TXT(rr::rdata::TXT::new(vec!["payload".into()])),
        ));
        let mut edns = Edns::new();
        edns.set_max_payload(1232).set_dnssec_ok(true);
        *message.extensions_mut() = Some(edns);
        let mut shared = DnsResponse::from(message);
        shared.prepare_wire();
        for code in [
            ResponseCode::NoError,
            ResponseCode::ServFail,
            ResponseCode::from(1, 7),
        ] {
            let mut header = Header::new(4321, MessageType::Response, OpCode::Query);
            header
                .set_response_code(code)
                .set_recursion_available(true)
                .set_authentic_data(true)
                .set_checking_disabled(true);
            let packet = SerialMessage::reply(
                shared.clone(),
                header,
                "127.0.0.1:5300".parse().unwrap(),
                Protocol::Udp,
            );
            let bytes = Vec::<u8>::try_from(packet).unwrap();
            let decoded = Message::from_vec(&bytes).unwrap();
            assert_eq!(decoded.id(), 4321);
            assert_eq!(decoded.response_code(), code);
            assert!(
                decoded.recursion_available()
                    && decoded.authentic_data()
                    && decoded.checking_disabled()
            );
            assert_eq!(decoded.queries(), shared.queries());
            assert_eq!(decoded.answers(), shared.answers());
            assert_eq!(decoded.authorities(), shared.authorities());
            assert_eq!(decoded.additionals(), shared.additionals());
            assert_eq!(decoded.extensions().as_ref().unwrap().max_payload(), 1232);
            assert!(decoded.extensions().as_ref().unwrap().flags().dnssec_ok);
            assert_eq!(shared.id(), 7);
            assert_eq!(shared.response_code(), ResponseCode::NoError);
        }
    }
}
