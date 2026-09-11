use crate::dns::{DefaultSOA as _, DnsResponse};
use crate::libdns::proto::{
    AuthorityData, NoRecords, ProtoError, ProtoErrorKind,
    op::{Query, ResponseCode},
    rr::{Record, rdata::SOA},
};
use std::{io, sync::Arc};
use thiserror::Error;

#[allow(clippy::large_enum_variant)]
/// A query could not be fulfilled
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum LookupError {
    /// A record at the same Name as the query exists, but not of the queried RecordType
    #[error("The name exists, but not for the record requested")]
    NameExists,
    /// There was an error performing the lookup
    #[error("Error performing lookup: {0}")]
    ResponseCode(ResponseCode),
    /// An error got returned by the hickory-proto crate
    #[error("proto error: {0}")]
    Proto(#[from] ProtoError),
    /// An underlying IO error occurred
    #[error("io error: {0}")]
    Io(Arc<io::Error>),
}

impl PartialEq for LookupError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ResponseCode(l0), Self::ResponseCode(r0)) => l0 == r0,
            (Self::Proto(l0), Self::Proto(r0)) => l0.to_string() == r0.to_string(),
            (Self::Io(l0), Self::Io(r0)) => l0.to_string() == r0.to_string(),
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
}

impl LookupError {
    /// Whether this is a valid NODATA or NXDOMAIN response, rather than an upstream failure.
    pub fn is_negative_response(&self) -> bool {
        self.is_nx_domain()
            || matches!(
                self,
                Self::Proto(err)
                    if matches!(
                        err.kind(),
                        ProtoErrorKind::NoRecordsFound(NoRecords {
                            response_code: ResponseCode::NoError,
                            ..
                        })
                    )
            )
    }

    pub fn is_nx_domain(&self) -> bool {
        matches!(self, Self::ResponseCode(resc) if resc.eq(&ResponseCode::NXDomain))
            || matches!(
                self,
                Self::Proto(err)
                    if matches!(
                        err.kind(),
                        ProtoErrorKind::NoRecordsFound(NoRecords { response_code, .. })
                            if response_code == &ResponseCode::NXDomain
                    )
            )
    }

    #[inline]
    pub fn is_soa(&self) -> bool {
        if let Self::Proto(err) = self
            && let ProtoErrorKind::NoRecordsFound(NoRecords { soa: Some(_), .. }) = err.kind()
        {
            return true;
        }
        false
    }

    pub fn as_soa(&self, query: &Query) -> Option<DnsResponse> {
        if let Self::Proto(err) = self
            && let ProtoErrorKind::NoRecordsFound(NoRecords {
                soa: Some(record), ..
            }) = err.kind()
        {
            let mut dns_response = DnsResponse::new_with_max_ttl(query.to_owned(), Vec::new());
            dns_response.add_authority(record.as_ref().to_owned().into_record_of_rdata());
            return Some(dns_response);
        }
        None
    }

    pub fn as_no_records_response(&self, query: &Query) -> Option<DnsResponse> {
        if let Self::Proto(err) = self
            && let ProtoErrorKind::NoRecordsFound(NoRecords {
                soa, response_code, ..
            }) = err.kind()
        {
            let mut response = DnsResponse::new_with_max_ttl(query.to_owned(), Vec::new());
            response.set_response_code(*response_code);
            if let Some(record) = soa {
                response.add_authority(record.as_ref().to_owned().into_record_of_rdata());
            }
            return Some(response);
        }
        None
    }

    pub fn no_records_found(query: Query, ttl: u32) -> LookupError {
        let soa = Record::from_rdata(query.name().to_owned(), ttl, SOA::default_soa());

        let no_records = AuthorityData::new(query.into(), Some(Box::new(soa)), true, true, None);
        let mut no_records: NoRecords = no_records.into();
        no_records.response_code = ResponseCode::ServFail;

        ProtoErrorKind::NoRecordsFound(no_records).into()
    }
}

impl From<ResponseCode> for LookupError {
    fn from(value: ResponseCode) -> Self {
        Self::ResponseCode(value)
    }
}

impl From<ProtoErrorKind> for LookupError {
    fn from(value: ProtoErrorKind) -> Self {
        Self::Proto(value.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libdns::proto::rr::{Name, RecordType};
    use std::str::FromStr as _;

    #[test]
    fn no_records_without_soa_becomes_an_empty_success_response() {
        let query = Query::query(
            Name::from_str("no-https-record.example.").unwrap(),
            RecordType::HTTPS,
        );
        let authority = AuthorityData::new(Box::new(query.clone()), None, true, false, None);
        let err: LookupError = ProtoErrorKind::NoRecordsFound(authority.into()).into();

        let response = err
            .as_no_records_response(&query)
            .expect("NoRecordsFound should produce a DNS response even without an SOA");

        assert_eq!(response.response_code(), ResponseCode::NoError);
        assert!(response.answers().is_empty());
        assert!(response.authorities().is_empty());
    }
}
