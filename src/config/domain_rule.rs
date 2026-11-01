use crate::libdns::proto::rr::rdata::opt::ClientSubnet;

use super::*;

/// domain-rules /domain/ [-rules...]
#[derive(Debug, Clone, Default, Hash, PartialEq, Eq)]
pub struct DomainRule {
    /// The name of NameServer Group.
    pub nameserver: Option<String>,

    pub address: Option<AddressRuleValue>,

    pub cname: Option<CNameRule>,

    pub srv: Option<SRV>,

    pub txt: Option<Vec<TXT>>,

    pub https: Option<HttpsRecordRule>,

    /// The mode of speed checking.
    pub speed_check_mode: Option<SpeedCheckModeList>,

    pub dualstack_ip_selection: Option<bool>,

    pub response_mode: Option<ResponseMode>,

    pub no_cache: Option<bool>,
    pub no_serve_expired: Option<bool>,
    pub nftset: Option<Vec<ConfigForIP<NFTsetConfig>>>,
    pub ipset: Option<Vec<ConfigForIP<KernelIpSet>>>,

    pub rr_ttl: Option<u64>,
    pub rr_ttl_min: Option<u64>,
    pub rr_ttl_max: Option<u64>,

    pub subnet: Option<ClientSubnet>,

    /// The group that repeats an address lookup when the answer from this
    /// rule's `nameserver` has an address in `recheck_ip_set`.
    pub recheck_nameserver: Option<String>,

    /// The ip-set whose addresses send an answer to `recheck_nameserver`.
    pub recheck_ip_set: Option<String>,
}

impl DomainRule {
    /// This rule with each option it does not set taken from `parent`, the
    /// rule of a parent domain, as C SmartDNS takes every option from the
    /// most specific rule that sets it.
    ///
    /// Like the TTL rule slot in C SmartDNS, `rr-ttl`, `rr-ttl-min` and
    /// `rr-ttl-max` are inherited only together. The recheck options go
    /// with the nameserver: a rule that sets its own `nameserver` keeps
    /// its own recheck, if any.
    pub fn inherit(&self, parent: &DomainRule) -> DomainRule {
        let mut rule = self.clone();

        macro_rules! fill {
            ($($field:ident),+) => {$(
                if rule.$field.is_none() {
                    rule.$field = parent.$field.clone();
                }
            )+};
        }

        if rule.nameserver.is_none() {
            fill!(recheck_nameserver, recheck_ip_set);
        }
        if rule.rr_ttl.is_none() && rule.rr_ttl_min.is_none() && rule.rr_ttl_max.is_none() {
            fill!(rr_ttl, rr_ttl_min, rr_ttl_max);
        }
        fill!(
            nameserver,
            address,
            cname,
            srv,
            txt,
            https,
            speed_check_mode,
            dualstack_ip_selection,
            response_mode,
            no_cache,
            no_serve_expired,
            nftset,
            ipset,
            subnet
        );
        rule
    }
}

impl std::ops::AddAssign for DomainRule {
    fn add_assign(&mut self, rhs: Self) {
        if let Some(txt) = rhs.txt {
            self.txt.get_or_insert_with(Vec::new).extend(txt);
        }
        if rhs.nameserver.is_some() {
            self.nameserver = rhs.nameserver;
        }

        if rhs.address.is_some() {
            self.address = rhs.address;
        }
        if rhs.cname.is_some() {
            self.cname = rhs.cname;
        }
        if rhs.srv.is_some() {
            self.srv = rhs.srv;
        }
        if rhs.https.is_some() {
            self.https = rhs.https;
        }
        if rhs.subnet.is_some() {
            self.subnet = rhs.subnet;
        }

        if rhs.speed_check_mode.is_some() {
            self.speed_check_mode = rhs.speed_check_mode;
        }
        if rhs.dualstack_ip_selection.is_some() {
            self.dualstack_ip_selection = rhs.dualstack_ip_selection;
        }
        if rhs.response_mode.is_some() {
            self.response_mode = rhs.response_mode;
        }
        if rhs.no_cache.is_some() {
            self.no_cache = rhs.no_cache;
        }
        if rhs.no_serve_expired.is_some() {
            self.no_serve_expired = rhs.no_serve_expired
        }
        if rhs.nftset.is_some() {
            self.nftset = rhs.nftset;
        }
        self.ipset = rhs.ipset.or(self.ipset.take());

        if rhs.rr_ttl.is_some() {
            self.rr_ttl = rhs.rr_ttl;
        }
        if rhs.rr_ttl_min.is_some() {
            self.rr_ttl_min = rhs.rr_ttl_min;
        }

        self.rr_ttl_max = rhs.rr_ttl_max.or(self.rr_ttl_max);

        if rhs.recheck_nameserver.is_some() {
            self.recheck_nameserver = rhs.recheck_nameserver;
        }
        if rhs.recheck_ip_set.is_some() {
            self.recheck_ip_set = rhs.recheck_ip_set;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recheck_merges_into_a_rule_with_a_nameserver() {
        let mut rule = DomainRule {
            nameserver: Some("remote".to_string()),
            ..Default::default()
        };
        rule += DomainRule {
            recheck_nameserver: Some("china".to_string()),
            recheck_ip_set: Some("cn".to_string()),
            ..Default::default()
        };

        assert_eq!(rule.nameserver.as_deref(), Some("remote"));
        assert_eq!(rule.recheck_nameserver.as_deref(), Some("china"));
        assert_eq!(rule.recheck_ip_set.as_deref(), Some("cn"));
    }

    #[test]
    fn test_subnet_and_cname_survive_merges() {
        // Rules are built by merging each line into an empty rule, so an
        // option the merge skips is lost even on a single line.
        let subnet: ClientSubnet = "27.42.97.0/24".parse().unwrap();
        let cname = CNameRule::Value("cdn.example.net.".parse().unwrap());
        let mut rule = DomainRule::default();
        rule += DomainRule {
            nameserver: Some("remote".to_string()),
            subnet: Some(subnet),
            cname: Some(cname.clone()),
            ..Default::default()
        };
        rule += DomainRule {
            recheck_nameserver: Some("china".to_string()),
            ..Default::default()
        };

        assert_eq!(rule.subnet, Some(subnet));
        assert_eq!(rule.cname, Some(cname));
        assert_eq!(rule.nameserver.as_deref(), Some("remote"));
    }
}
