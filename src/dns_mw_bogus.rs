use std::ops::Deref;

use crate::libdns::proto::op::ResponseCode;

use crate::dns::*;
use crate::dns_conf::RuntimeConfig;

use crate::middleware::*;

pub struct DnsBogusMiddleware;

#[async_trait::async_trait]
impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for DnsBogusMiddleware {
    async fn handle(
        &self,
        ctx: &mut DnsContext,
        req: &DnsRequest,
        next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
    ) -> Result<DnsResponse, DnsError> {
        let res = next.run(ctx, req).await;

        if req.query().query_type().is_ip_addr()
            && let Ok(lookup) = res.as_ref()
            && has_bogus_address(ctx.cfg(), lookup)
        {
            return Err(ResponseCode::NXDomain.into());
        }
        res
    }
}

/// Whether an answer contains a `bogus-nxdomain` address.
pub(crate) fn has_bogus_address(cfg: &RuntimeConfig, lookup: &DnsResponse) -> bool {
    let bogus_nxdomain = cfg.bogus_nxdomain();

    lookup.records().iter().any(|record| match record.data() {
        RData::A(ip) => bogus_nxdomain.contains(ip.deref()),
        RData::AAAA(ip) => bogus_nxdomain.contains(ip.deref()),
        _ => false,
    })
}
