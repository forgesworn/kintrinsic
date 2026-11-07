//! Strict JSON for guardian-key content (statements, presence confirmations,
//! pair offers, rosters).
//!
//! Every ward must read one signed event the same way, whatever its JSON
//! library, so on top of RFC 8259 this parser refuses:
//! - a duplicate member name in any object (serde_json keeps the last copy;
//!   other libraries keep the first, or throw);
//! - any number token that is not an integer: no fraction, no exponent, no
//!   `-0`;
//! - a lone UTF-16 surrogate in any string (a raw one cannot occur in a Rust
//!   `&str`, so only the `\uD800` escape form is reachable here);
//! - nesting deeper than [`MAX_DEPTH`] objects and arrays.
//!
//! Integers beyond the `i64`/`u64` range still parse (they may sit in ignored
//! fields) and become an imprecise float; every field the contract reads is
//! range-checked separately, so such a value is always refused there.

use serde_json::{Map, Number, Value};

/// Maximum nesting of objects and arrays; a top-level object is depth 1.
pub const MAX_DEPTH: usize = 16;

/// Why a document was refused. The position is a byte offset, for debugging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrictJsonError {
    pub message: &'static str,
    pub at: usize,
}

impl std::fmt::Display for StrictJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}", self.message, self.at)
    }
}

impl std::error::Error for StrictJsonError {}

/// Parse `text` as strict JSON.
pub fn parse_strict_json(text: &str) -> Result<Value, StrictJsonError> {
    let mut p = Parser {
        b: text.as_bytes(),
        text,
        i: 0,
    };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.fail("trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    text: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn fail(&self, message: &'static str) -> StrictJsonError {
        StrictJsonError {
            message,
            at: self.i,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, StrictJsonError> {
        self.ws();
        match self.peek() {
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.fail("unexpected character")),
        }
    }

    fn literal(&mut self, word: &str, v: Value) -> Result<Value, StrictJsonError> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.fail("bad literal"))
        }
    }

    fn number(&mut self) -> Result<Value, StrictJsonError> {
        let start = self.i;
        let mut j = self.i;
        if self.b.get(j) == Some(&b'-') {
            j += 1;
        }
        match self.b.get(j) {
            Some(b'0') => j += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.b.get(j), Some(b'0'..=b'9')) {
                    j += 1;
                }
            }
            _ => return Err(self.fail("bad number")),
        }
        if matches!(self.b.get(j), Some(b'.' | b'e' | b'E')) {
            return Err(self.fail("non-integer number"));
        }
        let tok = &self.text[start..j];
        if tok == "-0" {
            return Err(self.fail("negative zero"));
        }
        self.i = j;
        let n = if let Ok(n) = tok.parse::<i64>() {
            Number::from(n)
        } else if let Ok(n) = tok.parse::<u64>() {
            Number::from(n)
        } else {
            // Out of range: keep an imprecise float, as a JS reader would. A
            // finite stand-in for an overflow keeps it a number.
            let f: f64 = tok.parse().unwrap_or(0.0);
            let f = if f.is_finite() {
                f
            } else {
                f64::MAX.copysign(f)
            };
            Number::from_f64(f).ok_or_else(|| self.fail("bad number"))?
        };
        Ok(Value::Number(n))
    }

    fn hex4(&mut self) -> Result<u16, StrictJsonError> {
        let s = self
            .b
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.fail("bad unicode escape"))?;
        if !s.iter().all(u8::is_ascii_hexdigit) {
            return Err(self.fail("bad unicode escape"));
        }
        let s = std::str::from_utf8(s).map_err(|_| self.fail("bad unicode escape"))?;
        let v = u16::from_str_radix(s, 16).map_err(|_| self.fail("bad unicode escape"))?;
        self.i += 4;
        Ok(v)
    }

    /// A string token, decoded as UTF-16 code units and then checked for lone
    /// surrogates, exactly as a JS reader sees it.
    fn string(&mut self) -> Result<String, StrictJsonError> {
        self.i += 1; // opening quote
        let mut out: Vec<u16> = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.fail("unterminated string"));
            };
            if c == b'"' {
                self.i += 1;
                break;
            }
            if c < 0x20 {
                return Err(self.fail("control character in string"));
            }
            if c == b'\\' {
                let e = self.b.get(self.i + 1).copied();
                self.i += 2;
                let unit = match e {
                    Some(b'"') => 0x22,
                    Some(b'\\') => 0x5c,
                    Some(b'/') => 0x2f,
                    Some(b'b') => 0x08,
                    Some(b'f') => 0x0c,
                    Some(b'n') => 0x0a,
                    Some(b'r') => 0x0d,
                    Some(b't') => 0x09,
                    Some(b'u') => self.hex4()?,
                    _ => return Err(self.fail("bad escape")),
                };
                out.push(unit);
                continue;
            }
            // One whole UTF-8 character (the input is a valid &str).
            let ch = self.text[self.i..]
                .chars()
                .next()
                .ok_or_else(|| self.fail("unterminated string"))?;
            let mut buf = [0u16; 2];
            out.extend_from_slice(ch.encode_utf16(&mut buf));
            self.i += ch.len_utf8();
        }
        String::from_utf16(&out).map_err(|_| self.fail("lone surrogate"))
    }

    fn array(&mut self, depth: usize) -> Result<Value, StrictJsonError> {
        if depth > MAX_DEPTH {
            return Err(self.fail("nested too deeply"));
        }
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Array(out));
        }
        loop {
            out.push(self.value(depth)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Array(out));
                }
                _ => return Err(self.fail("bad array")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, StrictJsonError> {
        if depth > MAX_DEPTH {
            return Err(self.fail("nested too deeply"));
        }
        self.i += 1;
        let mut out = Map::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Object(out));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.fail("expected member name"));
            }
            let k = self.string()?;
            if out.contains_key(&k) {
                return Err(self.fail("duplicate member name"));
            }
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.fail("expected colon"));
            }
            self.i += 1;
            let v = self.value(depth)?;
            out.insert(k, v);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Object(out));
                }
                _ => return Err(self.fail("bad object")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> bool {
        parse_strict_json(s).is_ok()
    }

    #[test]
    fn accepts_and_refuses() {
        assert!(ok(r#"{"v":1,"a":[1,-2,"xé😀"],"b":null,"c":true}"#));
        assert!(ok("123456789012345678901234567890"));
        assert!(!ok(r#"{"a":1,"a":2}"#));
        assert!(!ok(r#"{"x":{"a":1,"a":2}}"#));
        assert!(!ok("1.0"));
        assert!(!ok("1e3"));
        assert!(!ok("-0"));
        assert!(!ok("01"));
        assert!(!ok(r#""\ud800""#));
        assert!(!ok(r#""a\udc00""#));
        assert!(!ok(r#"{"a":1,}"#));
        assert!(!ok("\"a\u{1}\""));
        assert!(!ok(""));
    }

    #[test]
    fn duplicate_names_compare_decoded() {
        assert!(!ok(r#"{"a":1,"a":2}"#));
    }

    #[test]
    fn depth_limit() {
        let nest = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(ok(&nest(MAX_DEPTH)));
        assert!(!ok(&nest(MAX_DEPTH + 1)));
        assert!(!ok(&nest(100_000)));
        let obj = |n: usize| format!("{}1{}", r#"{"a":"#.repeat(n), "}".repeat(n));
        assert!(ok(&obj(MAX_DEPTH)));
        assert!(!ok(&obj(MAX_DEPTH + 1)));
    }
}
