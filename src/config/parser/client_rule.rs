use super::*;

impl NomParser for ClientRule {
    fn parse(input: &str) -> IResult<&str, Self> {
        let (input, client) = NomParser::parse(input)?;
        let (rest_input, options) = opt(preceded(space1, options::parse)).parse(input)?;
        let options = options.unwrap_or_default();
        let (_, mut opts) = bind_addr::parse_server_opts(&options).map_err(|_| {
            nom::Err::Failure(nom::error::Error::new(input, nom::error::ErrorKind::Verify))
        })?;
        let group = opts
            .group
            .take()
            .or_else(|| {
                options
                    .iter()
                    .find(|(k, _)| *k == "g")
                    .and_then(|(_, v)| v.map(str::to_string))
            })
            .unwrap_or_default();
        Ok((
            rest_input,
            ClientRule {
                group,
                client,
                options: opts,
            },
        ))
    }
}

impl NomParser for Client {
    fn parse(input: &str) -> IResult<&str, Self> {
        alt((
            map(NomParser::parse, Client::IpAddr),
            map(nom_recipes::mac_addr, |mac| {
                Client::MacAddr(mac.to_string())
            }),
        ))
        .parse(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        let (_, rule) = ClientRule::parse("01:23:45:67:89:ab -group kids -no-cache -no-speed-check -no-serve-expired -nftset #4:inet#fw4#kids").unwrap();
        assert_eq!(rule.group, "kids");
        assert!(rule.options.no_cache());
        assert!(rule.options.no_speed_check());
        assert!(rule.options.no_serve_expired());
        assert!(
            matches!(&rule.options.nftset.unwrap()[0], ConfigForIP::V4(set) if set.name == "kids")
        );
        assert_eq!(
            ClientRule::parse("01:23:45:67:89:ab -group a"),
            Ok((
                "",
                ClientRule {
                    options: Default::default(),
                    group: "a".to_string(),
                    client: Client::MacAddr("01:23:45:67:89:ab".to_string())
                }
            ))
        );

        assert_eq!(
            ClientRule::parse("192.168.0.0/16 --group a"),
            Ok((
                "",
                ClientRule {
                    options: Default::default(),
                    group: "a".to_string(),
                    client: Client::IpAddr("192.168.0.0/16".parse().unwrap())
                }
            ))
        );

        assert_eq!(
            ClientRule::parse("192.168.100.0/24"),
            Ok((
                "",
                ClientRule {
                    options: Default::default(),
                    group: "".to_string(),
                    client: Client::IpAddr("192.168.100.0/24".parse().unwrap())
                }
            ))
        );
    }
}
