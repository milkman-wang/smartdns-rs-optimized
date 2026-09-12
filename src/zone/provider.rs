use crate::dns::{DnsContext, DnsError, DnsRequest, DnsResponse};

pub trait ZoneProvider: Send + Sync {
    fn lookup(&self, ctx: &DnsContext, req: &DnsRequest) -> Result<Option<DnsResponse>, DnsError>;
}
