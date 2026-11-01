//! The few X.509 certificate fields SmartDNS reads, straight from DER.

use time::OffsetDateTime;

/// A DER element and the input after it.
struct Element<'a> {
    tag: u8,
    contents: &'a [u8],
    /// The element with its tag and length.
    whole: &'a [u8],
    rest: &'a [u8],
}

/// The DER element at the start of `input`.
fn element(input: &[u8]) -> Option<Element<'_>> {
    let (&tag, after_tag) = input.split_first()?;
    let (&first, mut rest) = after_tag.split_first()?;
    let len = if first < 0x80 {
        first as usize
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        let (bytes, after) = rest.split_at(count);
        rest = after;
        bytes.iter().fold(0, |len, &b| (len << 8) | b as usize)
    };
    if rest.len() < len {
        return None;
    }
    let header = input.len() - rest.len();
    let (contents, rest) = rest.split_at(len);
    Some(Element {
        tag,
        contents,
        whole: &input[..header + len],
        rest,
    })
}

/// The element at the start of `input` if it has `tag`.
fn expect(input: &[u8], tag: u8) -> Option<Element<'_>> {
    element(input).filter(|element| element.tag == tag)
}

/// Field `index` of the TBSCertificate after its version: serialNumber,
/// signature, issuer, validity, subject, subjectPublicKeyInfo, ...
fn tbs_field(der: &[u8], index: usize) -> Option<Element<'_>> {
    let certificate = expect(der, 0x30)?.contents;
    let mut fields = expect(certificate, 0x30)?.contents;
    if fields.first() == Some(&0xa0) {
        fields = expect(fields, 0xa0)?.rest;
    }
    for _ in 0..index {
        fields = element(fields)?.rest;
    }
    element(fields)
}

/// The notBefore and notAfter times of a certificate.
pub fn validity(der: &[u8]) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let validity = tbs_field(der, 3).filter(|field| field.tag == 0x30)?;
    let (not_before, rest) = time(validity.contents)?;
    let (not_after, _) = time(rest)?;
    Some((not_before, not_after))
}

/// The DER of a certificate's SubjectPublicKeyInfo, which SPKI pins hash.
pub fn subject_public_key_info(der: &[u8]) -> Option<&[u8]> {
    let spki = tbs_field(der, 5).filter(|field| field.tag == 0x30)?;
    Some(spki.whole)
}

/// A UTCTime or GeneralizedTime in the form RFC 5280 requires, and the
/// input after it.
fn time(input: &[u8]) -> Option<(OffsetDateTime, &[u8])> {
    let Element {
        tag,
        contents: value,
        rest,
        ..
    } = element(input)?;
    let year_digits = match tag {
        0x17 => 2, // UTCTime: YYMMDDHHMMSSZ
        0x18 => 4, // GeneralizedTime: YYYYMMDDHHMMSSZ
        _ => return None,
    };
    let text = std::str::from_utf8(value).ok()?.strip_suffix('Z')?;
    if text.len() != year_digits + 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text[range].parse::<u32>().ok();
    let year = match (year_digits, number(0..year_digits)?) {
        (2, year) if year >= 50 => 1900 + year,
        (2, year) => 2000 + year,
        (_, year) => year,
    };
    let field = |i: usize| number(year_digits + 2 * i..year_digits + 2 * i + 2);
    let date = time::Date::from_calendar_date(
        year as i32,
        time::Month::try_from(field(0)? as u8).ok()?,
        field(1)? as u8,
    )
    .ok()?;
    let time = time::Time::from_hms(field(2)? as u8, field(3)? as u8, field(4)? as u8).ok()?;
    Some((time::PrimitiveDateTime::new(date, time).assume_utc(), rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};

    #[test]
    fn test_fields_match_x509_parser() {
        let key = KeyPair::generate().unwrap();
        for (not_before, not_after) in [
            ((2020, 1, 2), (2049, 12, 31)), // UTCTime
            ((2045, 6, 7), (2090, 1, 1)),   // GeneralizedTime from 2050
        ] {
            let mut params = CertificateParams::new(vec!["example.com".into()]).unwrap();
            let date = |(y, m, d): (i32, u8, u8)| {
                time::Date::from_calendar_date(y, time::Month::try_from(m).unwrap(), d)
                    .unwrap()
                    .with_hms(1, 2, 3)
                    .unwrap()
                    .assume_utc()
            };
            params.not_before = date(not_before);
            params.not_after = date(not_after);
            let cert = params.self_signed(&key).unwrap();
            let der = cert.der();

            let (_, parsed) = x509_parser::parse_x509_certificate(der).unwrap();
            let (from, to) = validity(der).unwrap();
            assert_eq!(
                (from.unix_timestamp(), to.unix_timestamp()),
                (
                    parsed.validity().not_before.timestamp(),
                    parsed.validity().not_after.timestamp()
                )
            );
            assert_eq!(subject_public_key_info(der), Some(parsed.public_key().raw));
        }
    }

    #[test]
    fn test_malformed_input() {
        assert_eq!(validity(&[]), None);
        assert_eq!(validity(b"\x30\x03\x02\x01\x01"), None);
        assert_eq!(subject_public_key_info(b"\x30\x82\xff\xff"), None);
        assert_eq!(time(b"\x17\x0d2601010000Z00"), None);
    }
}
