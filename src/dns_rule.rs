use std::{collections::HashMap, ops::Deref, sync::Arc};

use crate::{config::WildcardName, third_ext::HashCode};
use std::sync::LazyLock;

use crate::{
    collections::DomainMap,
    config::{
        AddressRules, CNameRules, ConfigForDomain, ConfigForIP, Domain, DomainRule, DomainRules,
        DomainSets, ForwardRules, HttpsRecords, NFTsetConfig, SrvRecords,
    },
};

static EMPTY: LazyLock<DomainRuleMap> = LazyLock::new(DomainRuleMap::default);

fn expand_domain<'a>(
    domain: &'a Domain,
    domain_sets: &'a DomainSets,
) -> impl Iterator<Item = &'a WildcardName> {
    let (name, set) = match domain {
        Domain::Name(name) => (Some(name), None),
        Domain::Set(name) => (None, domain_sets.get(name)),
    };
    name.into_iter().chain(set.into_iter().flatten())
}

fn update_rule<'a>(
    rules: &mut HashMap<&'a WildcardName, Arc<DomainRule>>,
    shared_rules: &mut HashMap<u64, Arc<DomainRule>>,
    name: &'a WildcardName,
    update: impl FnOnce(&mut DomainRule),
) {
    use std::collections::hash_map::Entry;

    let entry = rules.entry(name);
    let mut rule = match &entry {
        Entry::Occupied(entry) => entry.get().as_ref().clone(),
        Entry::Vacant(_) => DomainRule::default(),
    };
    update(&mut rule);
    // Intern while expanding the list: storing a complete rule for every
    // domain makes large Passwall lists expensive even when the rules agree.
    let rule = shared_rules
        .entry(rule.hash_code())
        .or_insert_with(|| Arc::new(rule))
        .clone();
    entry.insert_entry(rule);
}

#[derive(Default)]
pub struct DomainRuleMap {
    rules: DomainMap<Arc<DomainRuleTreeNode>>,
}

impl DomainRuleMap {
    pub fn empty() -> &'static Self {
        &EMPTY
    }
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        rule_map: &mut HashMap<u64, Arc<DomainRule>>,
        domain_rules: &DomainRules,
        address_rules: &AddressRules,
        forward_rules: &ForwardRules,
        domain_sets: &DomainSets,
        cnames: &CNameRules,
        srv_records: &SrvRecords,
        https_records: &HttpsRecords,
        nftsets: &Vec<ConfigForDomain<Vec<ConfigForIP<NFTsetConfig>>>>,
    ) -> Self {
        // Config and domain sets outlive construction, so expansion and sorting
        // can borrow their names instead of retaining another full copy.
        let mut name_rule_map = HashMap::<&WildcardName, Arc<DomainRule>>::new();

        // append domain_rules
        for rule in domain_rules {
            for name in expand_domain(&rule.domain, domain_sets) {
                update_rule(&mut name_rule_map, rule_map, name, |value| {
                    *value += rule.config.clone();
                });
            }
        }

        // append address rule
        for rule in address_rules.iter() {
            for name in expand_domain(&rule.domain, domain_sets) {
                update_rule(&mut name_rule_map, rule_map, name, |value| {
                    value.address = Some(rule.address.clone());
                });
            }
        }

        // append forward rule
        for rule in forward_rules.iter() {
            for name in expand_domain(&rule.domain, domain_sets) {
                update_rule(&mut name_rule_map, rule_map, name, |value| {
                    value.nameserver = Some(rule.nameserver.clone());
                });
            }
        }

        // set cname
        for rule in cnames {
            for name in expand_domain(&rule.domain, domain_sets) {
                update_rule(&mut name_rule_map, rule_map, name, |value| {
                    value.cname = Some(rule.config.clone());
                });
            }
        }

        // set srv
        for rule in srv_records {
            for name in expand_domain(&rule.domain, domain_sets) {
                update_rule(&mut name_rule_map, rule_map, name, |value| {
                    value.srv = Some(rule.config.clone());
                });
            }
        }

        // set https
        for rule in https_records {
            for name in expand_domain(&rule.domain, domain_sets) {
                update_rule(&mut name_rule_map, rule_map, name, |value| {
                    value.https = Some(rule.config.clone());
                });
            }
        }

        for rule in nftsets {
            for name in expand_domain(&rule.domain, domain_sets) {
                update_rule(&mut name_rule_map, rule_map, name, |value| {
                    value.nftset = Some(rule.config.clone());
                });
            }
        }

        let mut rule_items = name_rule_map.into_iter().collect::<Vec<_>>();
        rule_items.sort_by_key(|(name, ..)| *name);

        let mut rules = DomainMap::default();

        for (name, rule) in rule_items {
            let zone = rules.find(&name.base_name()).cloned();

            // DomainMap owns the matching index. Nodes only need the rule and
            // parent link; retaining each indexed name duplicates list storage.
            let node = DomainRuleTreeNode { rule, zone };
            rules.insert(name.clone(), node.into());
        }

        Self { rules }
    }
}

impl Deref for DomainRuleMap {
    type Target = DomainMap<Arc<DomainRuleTreeNode>>;

    fn deref(&self) -> &Self::Target {
        &self.rules
    }
}

#[derive(Debug)]
pub struct DomainRuleTreeNode {
    rule: Arc<DomainRule>,                 // www.example.com
    zone: Option<Arc<DomainRuleTreeNode>>, // example.com
}

impl DomainRuleTreeNode {
    pub fn zone(&self) -> Option<&Arc<DomainRuleTreeNode>> {
        self.zone.as_ref()
    }
}

pub trait DomainRuleGetter {
    fn get<T>(&self, f: impl Fn(&DomainRuleTreeNode) -> Option<T>) -> Option<T>;

    fn get_ref<T>(&self, f: impl Fn(&DomainRuleTreeNode) -> Option<&T>) -> Option<&T>;
}

impl DomainRuleGetter for DomainRuleTreeNode {
    fn get<T>(&self, f: impl Fn(&DomainRuleTreeNode) -> Option<T>) -> Option<T> {
        f(self).or_else(|| self.zone().and_then(|z| f(z)))
    }

    fn get_ref<T>(&self, f: impl Fn(&DomainRuleTreeNode) -> Option<&T>) -> Option<&T> {
        f(self).or_else(|| self.zone().and_then(|z| f(z)))
    }
}

impl<N: AsRef<DomainRuleTreeNode>> DomainRuleGetter for N {
    fn get<T>(&self, f: impl Fn(&DomainRuleTreeNode) -> Option<T>) -> Option<T> {
        self.as_ref().get(f)
    }

    fn get_ref<T>(&self, f: impl Fn(&DomainRuleTreeNode) -> Option<&T>) -> Option<&T> {
        self.as_ref().get_ref(f)
    }
}

impl DomainRuleGetter for Option<Arc<DomainRuleTreeNode>> {
    fn get<T>(&self, f: impl Fn(&DomainRuleTreeNode) -> Option<T>) -> Option<T> {
        self.as_deref().and_then(f)
    }

    fn get_ref<T>(&self, f: impl Fn(&DomainRuleTreeNode) -> Option<&T>) -> Option<&T> {
        self.as_deref().and_then(f)
    }
}

impl Deref for DomainRuleTreeNode {
    type Target = DomainRule;

    fn deref(&self) -> &Self::Target {
        self.rule.as_ref()
    }
}

#[cfg(feature = "experimental-trie")]
impl From<Name> for crate::collections::TrieKey<Name> {
    fn from(value: Name) -> Self {
        let mut keys = vec![];
        let labels = value.into_iter().collect::<Vec<_>>();
        for i in 0..labels.len() {
            keys.push(Name::from_labels(labels[i..].to_vec()).unwrap())
        }
        keys.push(Name::root());
        Self(keys)
    }
}

#[cfg(feature = "experimental-trie")]
impl From<&Name> for crate::collections::TrieKey<Name> {
    fn from(value: &Name) -> Self {
        let mut keys = vec![];
        let labels = value.into_iter().collect::<Vec<_>>();
        for i in 0..labels.len() {
            keys.push(Name::from_labels(labels[i..].to_vec()).unwrap())
        }
        keys.push(Name::root());
        Self(keys)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{AddressRule, AddressRuleValue};
    use std::{net::Ipv4Addr, ptr};

    #[test]
    fn shared_rules_keep_domain_overrides_independent() {
        let mut builder = crate::dns_conf::RuntimeConfig::builder();
        for domain in ["a.example", "b.example", "c.example"] {
            builder = builder.with(&format!(
                "domain-rules /{domain}/ -nameserver remote -rr-ttl-min 10"
            ));
        }
        let cfg = builder
            .with("domain-rules /a.example/ -nameserver domestic -rr-ttl-max 50")
            .with("nameserver /a.example/forward")
            .build()
            .unwrap();
        let a = cfg
            .find_domain_rule(&"a.example".parse().unwrap(), "default")
            .unwrap();
        let b = cfg
            .find_domain_rule(&"b.example".parse().unwrap(), "default")
            .unwrap();
        let c = cfg
            .find_domain_rule(&"c.example".parse().unwrap(), "default")
            .unwrap();

        assert_eq!(a.nameserver.as_deref(), Some("forward"));
        assert_eq!((a.rr_ttl_min, a.rr_ttl_max), (Some(10), Some(50)));
        assert_eq!(b.nameserver.as_deref(), Some("remote"));
        assert_eq!((b.rr_ttl_min, b.rr_ttl_max), (Some(10), None));
        assert!(std::sync::Arc::ptr_eq(&b.rule, &c.rule));
        assert!(!std::sync::Arc::ptr_eq(&a.rule, &b.rule));
    }

    use super::*;

    #[test]
    fn test_zone_rule() {
        let map = DomainRuleMap::create(
            &mut Default::default(),
            &Default::default(),
            &vec![
                AddressRule {
                    domain: "a.b.c.www.example.com".parse().unwrap(),
                    address: AddressRuleValue::Addr {
                        v4: Some([Ipv4Addr::new(127, 0, 0, 2)].into()),
                        v6: None,
                    },
                },
                AddressRule {
                    domain: "www.example.com".parse().unwrap(),
                    address: AddressRuleValue::Addr {
                        v4: Some([Ipv4Addr::LOCALHOST].into()),
                        v6: None,
                    },
                },
                AddressRule {
                    domain: "example.com".parse().unwrap(),
                    address: AddressRuleValue::Addr {
                        v4: Some([Ipv4Addr::LOCALHOST].into()),
                        v6: None,
                    },
                },
            ],
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
        );

        let rule1 = map.find(&"z.a.b.c.www.example.com".parse().unwrap());
        assert!(rule1.is_some());
        assert_eq!(
            rule1.and_then(|o| o.address.as_ref()),
            Some(&AddressRuleValue::Addr {
                v4: Some([Ipv4Addr::new(127, 0, 0, 2)].into()),
                v6: None,
            })
        );

        let rule2 = map.find(&"www.example.com".parse().unwrap());

        assert_eq!(
            rule2.and_then(|o| o.address.as_ref()),
            Some(&AddressRuleValue::Addr {
                v4: Some([Ipv4Addr::LOCALHOST].into()),
                v6: None,
            })
        );

        assert!(ptr::eq(
            rule1.as_ref().unwrap().zone().unwrap().as_ref(),
            rule2.unwrap().as_ref()
        ))
    }

    #[test]
    fn compact_nodes_keep_wildcards_and_parent_rules() {
        let cfg = crate::dns_conf::RuntimeConfig::builder()
            .with("domain-rules /example.com/ -nameserver parent -rr-ttl-min 17")
            .with("domain-rules /a*b.example.com/ -nameserver wildcard")
            .with("domain-rules /-.exact.example.com/ -nameserver exact")
            .with("domain-rules /+.suffix.example.com/ -nameserver suffix")
            .build()
            .unwrap();

        for (name, nameserver) in [
            ("aab.example.com", "wildcard"),
            ("deep.aab.example.com", "parent"),
            ("exact.example.com", "exact"),
            ("child.exact.example.com", "parent"),
            ("suffix.example.com", "parent"),
            ("child.suffix.example.com", "suffix"),
        ] {
            let rule = cfg
                .find_domain_rule(&name.parse().unwrap(), "default")
                .unwrap();
            assert_eq!(rule.nameserver.as_deref(), Some(nameserver), "{name}");
        }
        let exact = cfg
            .find_domain_rule(&"exact.example.com".parse().unwrap(), "default")
            .unwrap();
        assert_eq!(exact.get(|r| r.rr_ttl_min), Some(17));
    }
}
