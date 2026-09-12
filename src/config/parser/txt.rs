use super::*;

impl NomParser for TXT {
    fn parse(input: &str) -> IResult<&str, Self> {
        let (rest, text) = not_line_ending(input)?;
        let mut chunks = Vec::new();
        let mut chunk = String::new();
        let mut quoted = false;
        let mut escaped = false;
        let mut started = false;
        for c in text.chars() {
            if escaped {
                chunk.push(c);
                escaped = false;
                started = true;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = !quoted;
                started = true;
            } else if !quoted && c == '#' {
                break;
            } else if !quoted && c.is_whitespace() {
                if started {
                    chunks.push(std::mem::take(&mut chunk));
                    started = false;
                }
            } else {
                chunk.push(c);
                started = true;
            }
        }
        if quoted || escaped {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Verify,
            )));
        }
        if started || chunks.is_empty() {
            chunks.push(chunk);
        }
        if chunks.iter().any(|s| s.len() > 255) {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::TooLarge,
            )));
        }
        Ok((rest, TXT::new(chunks)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_txt_chunks_and_comments() {
        let (rest, value) = TXT::parse("\"hello world\" \"next\\\"chunk\" # comment").unwrap();
        assert!(rest.is_empty());
        assert_eq!(
            value
                .txt_data()
                .iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>(),
            vec![b"hello world".as_slice(), b"next\"chunk".as_slice()]
        );
        assert!(TXT::parse("\"unclosed").is_err());
        assert!(TXT::parse(&"x".repeat(256)).is_err());
    }
}
