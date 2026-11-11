//! Port of `github.com/docker/distribution/registry/client/auth/challenge`
//! (`ResponseChallenges` / `parseValueAndParams`).

use std::collections::BTreeMap;

use super::Response;

/// `challenge.Challenge`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Challenge {
    /// Lower-cased auth scheme (e.g. `bearer`).
    pub scheme: String,
    /// Lower-cased parameter names → values.
    pub parameters: BTreeMap<String, String>,
}

const IS_TOKEN: u8 = 1;
const IS_SPACE: u8 = 2;

fn octet_type(c: u8) -> u8 {
    let mut t = 0;
    let is_ctl = c <= 31 || c == 127;
    let is_char = c <= 127;
    let is_separator = b" \t\"(),/:;<=>?@[]\\{}".contains(&c);
    if b" \t\r\n".contains(&c) {
        t |= IS_SPACE;
    }
    if is_char && !is_ctl && !is_separator {
        t |= IS_TOKEN;
    }
    t
}

fn skip_space(s: &[u8]) -> &[u8] {
    let i = s.iter().position(|&c| octet_type(c) & IS_SPACE == 0).unwrap_or(s.len());
    &s[i..]
}

fn expect_token(s: &[u8]) -> (&[u8], &[u8]) {
    let i = s.iter().position(|&c| octet_type(c) & IS_TOKEN == 0).unwrap_or(s.len());
    (&s[..i], &s[i..])
}

fn expect_token_or_quoted(s: &[u8]) -> (Vec<u8>, &[u8]) {
    if !s.starts_with(b"\"") {
        let (t, r) = expect_token(s);
        return (t.to_vec(), r);
    }
    let s = &s[1..];
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'"' => return (s[..i].to_vec(), &s[i + 1..]),
            b'\\' => {
                let mut p: Vec<u8> = s[..i].to_vec();
                let mut escape = true;
                i += 1;
                while i < s.len() {
                    let b = s[i];
                    if escape {
                        escape = false;
                        p.push(b);
                    } else if b == b'\\' {
                        escape = true;
                    } else if b == b'"' {
                        return (p, &s[i + 1..]);
                    } else {
                        p.push(b);
                    }
                    i += 1;
                }
                return (Vec::new(), &[]);
            }
            _ => {}
        }
        i += 1;
    }
    (Vec::new(), &[])
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `parseValueAndParams`
pub fn parse_value_and_params(header: &str) -> (String, BTreeMap<String, String>) {
    let mut params = BTreeMap::new();
    let (value, s_rest) = expect_token(header.as_bytes());
    if value.is_empty() {
        return (String::new(), params);
    }
    let value = lossy(value).to_ascii_lowercase();
    let mut s: Vec<u8> = b",".to_vec();
    s.extend_from_slice(skip_space(s_rest));
    let mut rest: &[u8] = &s;
    while rest.starts_with(b",") {
        let (pkey, r) = expect_token(skip_space(&rest[1..]));
        if pkey.is_empty() {
            return (value, params);
        }
        if !r.starts_with(b"=") {
            return (value, params);
        }
        let (pvalue, r) = expect_token_or_quoted(&r[1..]);
        if pvalue.is_empty() {
            return (value, params);
        }
        params.insert(lossy(pkey).to_ascii_lowercase(), lossy(&pvalue));
        rest = skip_space(r);
    }
    (value, params)
}

/// Parse a list of `WWW-Authenticate` header values (`parseAuthHeader`).
pub fn parse_www_authenticate<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<Challenge> {
    values
        .into_iter()
        .filter_map(|h| {
            let (v, p) = parse_value_and_params(h);
            if v.is_empty() {
                None
            } else {
                Some(Challenge {
                    scheme: v,
                    parameters: p,
                })
            }
        })
        .collect()
}

/// `challenge.ResponseChallenges`: challenges of a 401 response.
pub fn response_challenges(resp: &Response) -> Vec<Challenge> {
    if resp.status != 401 {
        return Vec::new();
    }
    parse_www_authenticate(
        resp.headers
            .get_all(::http::header::WWW_AUTHENTICATE)
            .iter()
            .filter_map(|v| v.to_str().ok()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer() {
        let (v, p) = parse_value_and_params(
            r#"Bearer realm="https://auth.example.com/token",service="registry.example.com""#,
        );
        assert_eq!(v, "bearer");
        assert_eq!(p["realm"], "https://auth.example.com/token");
        assert_eq!(p["service"], "registry.example.com");
    }
}
