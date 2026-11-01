use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, DefaultHasher, Hasher},
};

use crate::{
    config::{Wildcard, WildcardName},
    libdns::proto::rr::Name,
};

/// What part of the name space a key matches, as in [`WildcardName`].
const FULL: u8 = 0;
const DEFAULT: u8 = 1;
const SUFFIX: u8 = 2;
const SUB: u8 = 3;

/// The maps' keys are already hashes, so they are used as they are.
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Only u64 keys are hashed; keep anything else well defined.
        for &byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(byte);
        }
    }

    #[inline]
    fn write_u64(&mut self, key: u64) {
        self.0 = key;
    }
}

type KeyMap<V> = HashMap<u64, V, BuildHasherDefault<KeyHasher>>;

/// A name in lowercase wire format without the root label. Each parent's
/// wire format is a suffix of it, so the parents are looked up without
/// building their names.
struct WireName {
    bytes: [u8; 255],
    len: usize,
    starts: [u8; 128],
    labels: usize,
}

impl WireName {
    fn new<'a>(labels: impl Iterator<Item = &'a [u8]>) -> Option<Self> {
        let mut name = Self {
            bytes: [0; 255],
            len: 0,
            starts: [0; 128],
            labels: 0,
        };
        for label in labels {
            let end = name.len + 1 + label.len();
            if end > name.bytes.len() || name.labels == name.starts.len() || label.len() > 63 {
                return None;
            }
            name.starts[name.labels] = name.len as u8;
            name.bytes[name.len] = label.len() as u8;
            let dst = &mut name.bytes[name.len + 1..end];
            dst.copy_from_slice(label);
            dst.make_ascii_lowercase();
            name.len = end;
            name.labels += 1;
        }
        Some(name)
    }

    /// The wire format of the name without its first `level` labels; past
    /// the last label, the root's.
    #[inline]
    fn parent(&self, level: usize) -> &[u8] {
        match self.starts[..self.labels].get(level) {
            Some(&start) => &self.bytes[start as usize..self.len],
            None => &[],
        }
    }
}

#[inline]
fn key(kind: u8, wire: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write_u8(kind);
    hasher.write(wire);
    hasher.finish()
}

fn kind(name: &WildcardName) -> u8 {
    match name {
        WildcardName::Default(_) => DEFAULT,
        WildcardName::Suffix(_) => SUFFIX,
        WildcardName::Sub(_, _) => SUB,
        WildcardName::Full(_) => FULL,
    }
}

pub struct DomainMap<T> {
    /// Default, suffix and full names, by kind and name.
    map: KeyMap<T>,
    /// `*` names, by the name they are one level below.
    subs: KeyMap<Vec<(Wildcard, T)>>,
}

impl<T> DomainMap<T> {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// The value of the most specific name that matches `name`: its full
    /// or default name, then for each parent, from the nearest, its `*`
    /// names (first parent only), suffix and default names. Names compare
    /// without case and trailing dot.
    pub fn find(&self, name: &Name) -> Option<&T> {
        if self.is_empty() {
            return None;
        }
        // A wildcard query name is matched as its parent.
        let skip = usize::from(name.is_wildcard());
        let wire = WireName::new(name.iter().skip(skip))?;
        let get = |kind, level| self.map.get(&key(kind, wire.parent(level)));

        if let Some(value) = get(FULL, 0).or_else(|| get(DEFAULT, 0)) {
            return Some(value);
        }
        for level in 1..=wire.labels {
            if level == 1
                && !self.subs.is_empty()
                && let Some(subs) = self.subs.get(&key(SUB, wire.parent(1)))
                && let Some(first) = name.iter().nth(skip)
                && let Some((_, value)) = subs.iter().find(|(w, _)| w.is_match(first))
            {
                return Some(value);
            }
            if let Some(value) = get(SUFFIX, level).or_else(|| get(DEFAULT, level)) {
                return Some(value);
            }
        }
        None
    }

    /// The value inserted for exactly this name and kind.
    pub fn get_exact(&self, name: &WildcardName) -> Option<&T> {
        let wire = WireName::new(name.iter())?;
        match name {
            WildcardName::Sub(wildcard, _) => self
                .subs
                .get(&key(SUB, wire.parent(0)))?
                .iter()
                .find(|(w, _)| w == wildcard)
                .map(|(_, value)| value),
            _ => self.map.get(&key(kind(name), wire.parent(0))),
        }
    }

    #[inline]
    pub fn contains(&self, domain: &Name) -> bool {
        self.find(domain).is_some()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.map.len() + self.subs.values().map(Vec::len).sum::<usize>()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty() && self.subs.is_empty()
    }

    /// Inserts a value, replacing and returning the one for the same name.
    /// A name longer than DNS allows is not inserted.
    pub fn insert(&mut self, name: impl Into<WildcardName>, value: T) -> Option<T> {
        let name: WildcardName = name.into();
        let wire = WireName::new(name.iter())?;
        match name {
            WildcardName::Sub(wildcard, _) => {
                let subs = self.subs.entry(key(SUB, wire.parent(0))).or_default();
                match subs.iter_mut().find(|(w, _)| *w == wildcard) {
                    Some((_, old)) => Some(std::mem::replace(old, value)),
                    None => {
                        subs.push((wildcard, value));
                        None
                    }
                }
            }
            name => self.map.insert(key(kind(&name), wire.parent(0)), value),
        }
    }

    pub fn remove(&mut self, name: &WildcardName) -> Option<T> {
        let wire = WireName::new(name.iter())?;
        match name {
            WildcardName::Sub(wildcard, _) => {
                let k = key(SUB, wire.parent(0));
                let subs = self.subs.get_mut(&k)?;
                let index = subs.iter().position(|(w, _)| w == wildcard)?;
                let (_, value) = subs.remove(index);
                if subs.is_empty() {
                    self.subs.remove(&k);
                }
                Some(value)
            }
            _ => self.map.remove(&key(kind(name), wire.parent(0))),
        }
    }
}

impl<T> Default for DomainMap<T> {
    #[inline]
    fn default() -> Self {
        Self {
            map: Default::default(),
            subs: Default::default(),
        }
    }
}

impl<T> FromIterator<(WildcardName, T)> for DomainMap<T> {
    fn from_iter<I: IntoIterator<Item = (WildcardName, T)>>(iter: I) -> Self {
        let mut map = Self::default();
        for (name, value) in iter {
            map.insert(name, value);
        }
        map
    }
}

impl<T> From<HashMap<WildcardName, T>> for DomainMap<T> {
    #[inline]
    fn from(value: HashMap<WildcardName, T>) -> Self {
        value.into_iter().collect()
    }
}

#[derive(Default)]
pub struct DomainSet(DomainMap<()>);

impl DomainSet {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn contains(&self, domain: &Name) -> bool {
        self.0.contains(domain)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, domain: impl Into<WildcardName>) -> bool {
        self.0.insert(domain, ()).is_none()
    }

    #[inline]
    pub fn remove(&mut self, domain: &WildcardName) -> bool {
        self.0.remove(domain).is_some()
    }
}

impl FromIterator<WildcardName> for DomainSet {
    fn from_iter<T: IntoIterator<Item = WildcardName>>(iter: T) -> Self {
        DomainSet(DomainMap::from_iter(
            iter.into_iter().map(|item| (item, ())),
        ))
    }
}

#[cfg(feature = "experimental-trie")]
mod trie {
    use crate::third_ext::AsSlice;
    use std::{
        collections::{HashMap, hash_map::RandomState},
        hash::{BuildHasher, Hash},
    };

    #[derive(Default)]
    pub struct TrieMap<K, V, S = RandomState>(HashMap<K, TrieNode<K, V, S>>);

    impl<K, V, S> TrieMap<K, V, S> {
        pub fn insert(&mut self, trie_key: impl Into<TrieKey<K>>, v: V) -> Option<V>
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            let mut keys = Into::<TrieKey<K>>::into(trie_key);
            if let Some(key) = keys.pop() {
                self.0.entry(key).or_default().insert(keys, v)
            } else {
                Some(v)
            }
        }

        pub fn get(&self, trie_key: impl AsSlice<K>) -> Option<&V>
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            let keys = trie_key.as_slice();
            keys.last()
                .and_then(|key| self.0.get(key))
                .and_then(|n| n.get(&keys[..keys.len() - 1]))
        }

        pub fn get_mut(&mut self, trie_key: impl AsSlice<K>) -> Option<&mut V>
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            let keys = trie_key.as_slice();
            keys.last()
                .and_then(|key| self.0.get_mut(key))
                .and_then(|n| n.get_mut(&keys[..keys.len() - 1]))
        }

        pub fn contains(&self, trie_key: impl AsSlice<K>) -> bool
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            self.get(trie_key).is_some()
        }

        pub fn degree(&self) -> usize {
            self.0
                .values()
                .map(|c| c.degree())
                .max()
                .unwrap_or_default()
        }

        pub fn len(&self) -> usize {
            self.0.values().map(|n| n.len()).sum()
        }
    }

    impl<K, V> TrieMap<K, V> {
        pub fn new() -> Self {
            Self(Default::default())
        }
    }

    pub struct TrieNode<K, V, S = RandomState> {
        value: Option<V>,
        children: HashMap<K, TrieNode<K, V, S>, S>,
    }

    impl<K, V, S: Default> Default for TrieNode<K, V, S> {
        fn default() -> Self {
            Self {
                value: Default::default(),
                children: HashMap::<K, TrieNode<K, V, S>, S>::default(),
            }
        }
    }

    impl<K, V, S> TrieNode<K, V, S> {
        pub fn insert(&mut self, trie_key: TrieKey<K>, v: V) -> Option<V>
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            let mut keys = trie_key;
            if let Some(key) = keys.pop() {
                self.children.entry(key).or_default().insert(keys, v)
            } else {
                self.value.replace(v)
            }
        }

        pub fn get_node(&self, keys: &[K]) -> Option<&Self>
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            if let Some(key) = keys.last() {
                self.children
                    .get(key)
                    .and_then(|n| n.get_node(&keys[..keys.len() - 1]))
            } else {
                Some(self)
            }
        }

        pub fn get_node_mut(&mut self, keys: &[K]) -> Option<&mut Self>
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            if let Some(key) = keys.last() {
                self.children
                    .get_mut(key)
                    .and_then(|n| n.get_node_mut(&keys[..keys.len() - 1]))
            } else {
                Some(self)
            }
        }

        pub fn get(&self, keys: &[K]) -> Option<&V>
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            self.get_node(keys).and_then(|n| n.value.as_ref())
        }

        pub fn get_mut(&mut self, keys: &[K]) -> Option<&mut V>
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            self.get_node_mut(keys).and_then(|n| n.value.as_mut())
        }

        pub fn contains(&self, keys: &[K]) -> bool
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            self.get(keys).is_some()
        }

        pub fn degree(&self) -> usize {
            1 + self
                .children
                .values()
                .map(|c| c.degree())
                .max()
                .unwrap_or_default()
        }

        pub fn len(&self) -> usize {
            (if self.value.is_some() { 1 } else { 0 })
                + self.children.values().map(|n| n.len()).sum::<usize>()
        }
    }

    /// TrieKey in reverse mode
    #[derive(Debug)]
    pub struct TrieKey<K>(pub Vec<K>);

    impl<K> std::ops::Deref for TrieKey<K> {
        type Target = Vec<K>;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl<K> std::ops::DerefMut for TrieKey<K> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }

    impl<K> AsSlice<K> for TrieKey<K> {
        fn as_slice(&self) -> &[K] {
            self.0.as_slice()
        }
    }

    pub struct TrieSet<K, S = RandomState>(TrieMap<K, (), S>);

    impl<K> TrieSet<K> {
        #[inline]
        pub fn new() -> Self {
            Self(TrieMap::new())
        }
    }

    impl<K, S> TrieSet<K, S> {
        #[inline]
        pub fn insert(&mut self, trie_key: impl Into<TrieKey<K>>) -> bool
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            self.0.insert(trie_key, ()).is_none()
        }

        #[inline]
        pub fn contains(&self, trie_key: impl AsSlice<K>) -> bool
        where
            K: Eq + Hash,
            S: BuildHasher + Default,
        {
            self.0.contains(trie_key)
        }

        #[inline]
        pub fn len(&self) -> usize {
            self.0.len()
        }
    }
}
#[cfg(feature = "experimental-trie")]
pub use trie::*;

#[cfg(feature = "experimental-phf")]
mod phf {
    use std::{collections::HashMap, fmt::Debug, hash::Hash};

    use boomphf::hashmap::BoomHashMap;

    pub fn create_from_hashmap<K: Hash + Debug + PartialEq, V: Debug>(
        map: HashMap<K, V>,
    ) -> BoomHashMap<K, V> {
        let mut keys = vec![];
        let mut values = vec![];

        for (k, v) in map {
            keys.push(k);
            values.push(v);
        }
        BoomHashMap::new(keys, values)
    }
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_set_contains_default() {
        let mut set = DomainSet::new();

        set.insert(WildcardName::Default("example.com".parse().unwrap()));
        assert!(set.contains(&"example.com".parse().unwrap()));
        assert!(set.contains(&"www.example.com".parse().unwrap()));
        assert!(!set.contains(&"com".parse().unwrap()));
    }

    #[test]
    fn test_set_contains_suffix() {
        let mut set = DomainSet::new();

        set.insert(WildcardName::Suffix("example.com".parse().unwrap()));
        assert!(!set.contains(&"example.com".parse().unwrap()));
        assert!(set.contains(&"www.example.com".parse().unwrap()));
        assert!(set.contains(&"a.www.example.com".parse().unwrap()));
    }

    #[test]
    fn test_set_contains_sub() {
        let mut set = DomainSet::new();

        set.insert(WildcardName::Sub(
            Default::default(),
            "example.com".parse().unwrap(),
        ));
        assert!(!set.contains(&"example.com".parse().unwrap()));
        assert!(set.contains(&"www.example.com".parse().unwrap()));
        assert!(!set.contains(&"a.www.example.com".parse().unwrap()));
    }

    #[test]
    fn test_set_contains_sub2() {
        let mut set = DomainSet::new();

        set.insert(WildcardName::from_str("a*b.example.com").unwrap());

        assert!(set.contains(&"awwb.example.com".parse().unwrap()));
        assert!(!set.contains(&"aww.example.com".parse().unwrap()));
        assert!(!set.contains(&"wwb.example.com".parse().unwrap()));
        assert!(!set.contains(&"example.com".parse().unwrap()));
        assert!(!set.contains(&"www.example.com".parse().unwrap()));
        assert!(!set.contains(&"a.www.example.com".parse().unwrap()));
    }

    #[test]
    fn test_find_ignores_case_and_trailing_dot() {
        let mut map = DomainMap::new();
        map.insert(WildcardName::Default("Example.COM".parse().unwrap()), 1);

        assert_eq!(map.find(&"www.example.com.".parse().unwrap()), Some(&1));
        assert_eq!(map.find(&"EXAMPLE.com".parse().unwrap()), Some(&1));
        assert_eq!(map.find(&"example.org".parse().unwrap()), None);
    }

    #[test]
    fn test_find_root_and_wildcard_queries() {
        let mut map = DomainMap::new();
        map.insert(WildcardName::Default(Name::root()), 0);
        map.insert(WildcardName::Default("example.com".parse().unwrap()), 1);

        assert_eq!(map.find(&Name::root()), Some(&0));
        assert_eq!(map.find(&"example.org".parse().unwrap()), Some(&0));
        assert_eq!(map.find(&"a.b.example.com".parse().unwrap()), Some(&1));
        // A wildcard query name is matched as its parent.
        assert_eq!(map.find(&"*.com".parse().unwrap()), Some(&0));
        assert_eq!(map.find(&"*.example.com".parse().unwrap()), Some(&1));
    }

    #[test]
    fn test_get_exact_insert_and_remove() {
        let mut map = DomainMap::new();
        let names = [
            "example.com",
            "+.example.com",
            "*.example.com",
            "a*b.example.com",
            "-.example.com",
        ]
        .map(|name| WildcardName::from_str(name).unwrap());
        for (value, name) in names.iter().enumerate() {
            assert_eq!(map.insert(name.clone(), value), None);
        }
        assert_eq!(map.len(), names.len());
        for (value, name) in names.iter().enumerate() {
            assert_eq!(map.get_exact(name), Some(&value));
        }

        assert_eq!(map.insert(names[2].clone(), 10), Some(2));
        assert_eq!(map.remove(&names[2]), Some(10));
        assert_eq!(map.get_exact(&names[2]), None);
        assert_eq!(map.get_exact(&names[3]), Some(&3));
        assert_eq!(map.len(), names.len() - 1);
    }

    #[test]
    fn test_set_contains_full() {
        let mut set = DomainSet::new();

        set.insert(WildcardName::Full("example.com".parse().unwrap()));
        assert!(set.contains(&"example.com".parse().unwrap()));
        assert!(!set.contains(&"com".parse().unwrap()));
        assert!(!set.contains(&"www.example.com".parse().unwrap()));
        assert!(!set.contains(&"a.www.example.com".parse().unwrap()));
    }

    #[cfg(feature = "experimental-trie")]
    #[test]
    fn test_trie() {
        impl From<&str> for TrieKey<char> {
            fn from(value: &str) -> Self {
                Self(value.chars().rev().collect())
            }
        }
        let mut trie = TrieMap::new();
        assert_eq!(trie.insert("abc", "a"), None);
        assert_eq!(trie.get(TrieKey::from("abc").as_slice()), Some(&"a"));
        assert_eq!(trie.insert("abc", "w"), Some("a"));
        assert_eq!(trie.insert("axc", "w"), None);

        assert_eq!(trie.len(), 2);
        assert_eq!(trie.degree(), 3);

        assert_eq!(trie.get(TrieKey::from("ab")), None);
        assert_eq!(trie.get(TrieKey::from("abc")), Some(&"w"));
    }

    #[cfg(feature = "experimental-trie")]
    #[test]
    fn test_trie_domain_name() {
        let mut trie = TrieMap::new();

        assert_eq!(trie.insert(Name::from_str(".").unwrap(), "a"), None);

        assert_eq!(trie.insert(Name::from_str(".").unwrap(), "b"), Some("a"));

        assert_eq!(
            trie.insert(Name::from_str("example.com").unwrap(), "a"),
            None
        );

        assert_eq!(
            trie.insert(Name::from_str("Example.com").unwrap(), "b"),
            Some("a")
        );

        assert_eq!(
            trie.get(TrieKey::from(Name::from_str("Example.com").unwrap()).as_slice()),
            Some(&"b")
        );
    }
}

#[cfg(test)]
#[cfg(nightly)]
mod benchmark {
    //! rustup override set nightly
    //! rustup override unset
    extern crate test;
    use super::*;
    use crate::infra::http_client::{self, HttpResponse};
    use crate::libdns::proto::rr::Name;
    use std::{collections::HashSet, str::FromStr};

    fn get_domain_list() -> Vec<Name> {
        let mut domains = vec![];

        let text =
            http_client::get("https://raw.githubusercontent.com/StevenBlack/hosts/master/hosts")
                .unwrap()
                .text()
                .unwrap();

        for mut line in text.lines() {
            line = line.trim_start();
            if line.starts_with('#') {
                continue;
            };

            if let Some(name) = line.split(' ').nth(1).and_then(|s| Name::from_str(s).ok()) {
                domains.push(name);
            }
        }

        domains
    }

    #[cfg(feature = "experimental-trie")]
    #[test]
    fn test_trie_set() {
        let domain_list = get_domain_list().into_iter().collect::<HashSet<_>>();
        let domain_count = domain_list.len();
        let mut set = TrieSet::new();
        for name in get_domain_list() {
            set.insert(name);
        }
        assert_eq!(domain_count, set.len());
    }

    #[cfg(feature = "experimental-trie")]
    #[bench]
    fn bench_trie_set(b: &mut test::Bencher) {
        let domain_list = get_domain_list();
        let mut set = TrieSet::new();
        for name in domain_list.iter() {
            set.insert(name.clone());
        }
        let set = &set;
        let domain_list = domain_list
            .into_iter()
            .map(|n| TrieKey::from(n))
            .collect::<Vec<_>>();
        let domain_list = &domain_list;
        b.iter(|| {
            for n in domain_list {
                assert!(set.contains(n.as_slice()));
            }
        })
    }

    #[bench]
    fn bench_domain_set(b: &mut test::Bencher) {
        let domain_list = get_domain_list();
        let mut set = DomainSet::new();
        for name in domain_list.iter() {
            set.insert(name.clone());
        }
        let set = &set;
        let domain_list = &domain_list;
        b.iter(|| {
            for n in domain_list {
                assert!(set.contains(&n));
            }
        })
    }

    #[cfg(feature = "experimental-phf")]
    #[bench]
    fn bench_domain_phf(b: &mut test::Bencher) {
        let domain_list = get_domain_list();
        let mut set = DomainSet::new();
        for name in domain_list.iter() {
            set.insert(name.clone());
        }
        let set = phf::create_from_hashmap(set.0.0);
        let set = &set;

        let domain_list = &domain_list;
        b.iter(|| {
            for n in domain_list {
                assert!(set.get(&n).is_some());
            }
        })
    }
}
