use super::*;

impl NomParser for CertificateGeneration {
    fn parse(input: &str) -> IResult<&str, Self> {
        alt((
            value(Self::Auto, tag_no_case("auto")),
            map(bool::parse, |v| if v { Self::Yes } else { Self::No }),
        ))
        .parse(input)
    }
}

impl NomParser for Vec<String> {
    fn parse(input: &str) -> IResult<&str, Self> {
        separated_list1(
            space1,
            map(
                take_while1(|c: char| !c.is_whitespace() && c != '#'),
                str::to_string,
            ),
        )
        .parse(input)
    }
}
