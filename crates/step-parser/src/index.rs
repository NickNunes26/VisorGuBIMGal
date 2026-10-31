//! Single-pass entity indexer.

use std::collections::HashMap;

use crate::parser::Parser;
use crate::{Entry, ErrorKind, ParseError, StepIndex, Value};

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

struct Record {
    id: u32,
    type_name: String,
    start: usize,
    end: usize,
}

impl<'a> Scanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn error(&self, kind: ErrorKind) -> ParseError {
        ParseError::new(self.pos, kind)
    }

    fn error_at(&self, offset: usize, kind: ErrorKind) -> ParseError {
        ParseError::new(offset, kind)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<u8> {
        let at = self.pos.checked_add(ahead)?;
        self.bytes.get(at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos = self.pos.saturating_add(1);
        Some(byte)
    }

    fn skip_ws_comments(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                Some(byte) if byte.is_ascii_whitespace() => {
                    self.pos = self.pos.saturating_add(1);
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => self.skip_comment()?,
                _ => return Ok(()),
            }
        }
    }

    fn skip_comment(&mut self) -> Result<(), ParseError> {
        self.pos = self.pos.saturating_add(2);
        loop {
            match self.bump() {
                None => return Err(self.error(ErrorKind::UnterminatedComment)),
                Some(b'*') if self.peek() == Some(b'/') => {
                    self.pos = self.pos.saturating_add(1);
                    return Ok(());
                }
                Some(_) => {}
            }
        }
    }

    /// Skip the body of a single-quoted string. The opening quote is consumed.
    fn skip_single_quoted(&mut self) -> Result<(), ParseError> {
        loop {
            match self.bump() {
                None => return Err(self.error(ErrorKind::UnterminatedString)),
                Some(b'\'') => {
                    if self.peek() == Some(b'\'') {
                        self.pos = self.pos.saturating_add(1);
                    } else {
                        return Ok(());
                    }
                }
                Some(_) => {}
            }
        }
    }

    /// Skip a binary literal. The opening double quote is consumed.
    fn skip_double_quoted(&mut self) -> Result<(), ParseError> {
        loop {
            match self.bump() {
                None => return Err(self.error(ErrorKind::UnterminatedBinary)),
                Some(b'"') => return Ok(()),
                Some(_) => {}
            }
        }
    }

    fn read_word(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        let mut word = String::new();
        while let Some(byte) = self.peek() {
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
                word.push(byte.to_ascii_uppercase() as char);
                self.pos = self.pos.saturating_add(1);
            } else {
                break;
            }
        }
        if self.pos == start {
            Err(self.error(ErrorKind::InvalidIdentifier))
        } else {
            Ok(word)
        }
    }

    fn read_u32_digits(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let mut value: u64 = 0;
        while let Some(byte) = self.peek() {
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                _ => break,
            };
            value = value * 10 + u64::from(digit);
            if value > u64::from(u32::MAX) {
                return Err(self.error(ErrorKind::IdentifierOverflow));
            }
            self.pos = self.pos.saturating_add(1);
        }
        if self.pos == start {
            Err(self.error(ErrorKind::InvalidIdentifier))
        } else {
            Ok(value as u32)
        }
    }

    /// Scan `#id = TYPE(...);` without decoding its parameters.
    fn scan_entity(&mut self) -> Result<Record, ParseError> {
        let start = self.pos;
        if self.peek() != Some(b'#') {
            return Err(self.error(ErrorKind::MalformedEntity));
        }
        self.pos = self.pos.saturating_add(1);
        let id = self.read_u32_digits()?;

        self.skip_ws_comments()?;
        match self.bump() {
            Some(b'=') => {}
            Some(byte) => {
                return Err(self.error_at(
                    self.pos.saturating_sub(1),
                    ErrorKind::UnexpectedByte(byte),
                ));
            }
            None => return Err(self.error(ErrorKind::UnexpectedEof)),
        }

        self.skip_ws_comments()?;
        let type_name = self.read_word()?;
        self.skip_ws_comments()?;
        if self.peek() != Some(b'(') {
            return Err(self.error(ErrorKind::MalformedEntity));
        }
        self.pos = self.pos.saturating_add(1);

        // Find the record terminator while ignoring strings, comments and
        // nested parentheses. A semicolon outside them ends the entity.
        let mut depth = 1usize;
        loop {
            match self.peek() {
                None => return Err(self.error(ErrorKind::UnexpectedEof)),
                Some(b'\'') => {
                    self.pos = self.pos.saturating_add(1);
                    self.skip_single_quoted()?;
                }
                Some(b'"') => {
                    self.pos = self.pos.saturating_add(1);
                    self.skip_double_quoted()?;
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => self.skip_comment()?,
                Some(b'(') => {
                    self.pos = self.pos.saturating_add(1);
                    depth = depth.saturating_add(1);
                }
                Some(b')') => {
                    if depth == 0 {
                        return Err(self.error(ErrorKind::MalformedEntity));
                    }
                    self.pos = self.pos.saturating_add(1);
                    depth -= 1;
                }
                Some(b';') if depth == 0 => {
                    self.pos = self.pos.saturating_add(1);
                    break;
                }
                Some(_) => {
                    self.pos = self.pos.saturating_add(1);
                }
            }
        }

        Ok(Record {
            id,
            type_name,
            start,
            end: self.pos,
        })
    }

    /// Skip to and including the next top-level semicolon.
    fn skip_statement(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                None => return Err(self.error(ErrorKind::UnexpectedEof)),
                Some(b';') => {
                    self.pos = self.pos.saturating_add(1);
                    return Ok(());
                }
                Some(b'\'') => {
                    self.pos = self.pos.saturating_add(1);
                    self.skip_single_quoted()?;
                }
                Some(b'"') => {
                    self.pos = self.pos.saturating_add(1);
                    self.skip_double_quoted()?;
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => self.skip_comment()?,
                Some(_) => {
                    self.pos = self.pos.saturating_add(1);
                }
            }
        }
    }
}

pub(crate) fn build(bytes: &[u8]) -> Result<StepIndex<'_>, ParseError> {
    let mut scanner = Scanner::new(bytes);
    let mut schemas = Vec::new();

    let mut entries: Vec<Entry> = Vec::new();
    let mut by_id: HashMap<u32, u32> = HashMap::new();
    let mut types: Vec<String> = Vec::new();
    let mut type_lookup: HashMap<String, u32> = HashMap::new();
    let mut by_type: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut duplicates: Vec<u32> = Vec::new();

    loop {
        scanner.skip_ws_comments()?;
        let byte = match scanner.peek() {
            Some(byte) => byte,
            None => break,
        };

        if byte == b'#' {
            let record = scanner.scan_entity()?;

            if by_id.contains_key(&record.id) {
                duplicates.push(record.id);
                continue;
            }

            let type_idx = match type_lookup.get(&record.type_name).copied() {
                Some(idx) => idx,
                None => {
                    let idx = u32::try_from(types.len())
                        .map_err(|_| scanner.error(ErrorKind::IdentifierOverflow))?;
                    type_lookup.insert(record.type_name.clone(), idx);
                    types.push(record.type_name);
                    idx
                }
            };

            let entry_idx =
                u32::try_from(entries.len()).map_err(|_| scanner.error(ErrorKind::IdentifierOverflow))?;
            entries.push(Entry {
                id: record.id,
                type_idx,
                start: record.start,
                end: record.end,
            });
            by_id.insert(record.id, entry_idx);
            by_type.entry(type_idx).or_default().push(record.id);
            continue;
        }

        if byte == b';' {
            // Tolerate a stray separator between statements.
            scanner.pos = scanner.pos.saturating_add(1);
            continue;
        }

        let word = scanner.read_word()?;
        scanner.skip_ws_comments()?;

        if word == "FILE_SCHEMA" {
            if scanner.peek() != Some(b'(') {
                return Err(scanner.error(ErrorKind::MalformedEntity));
            }
            let mut parser = Parser::new(bytes, scanner.pos, bytes.len());
            let params = parser.parse_outer_params()?;
            collect_schemas(&params, &mut schemas);
            scanner.pos = parser.position();
        }

        scanner.skip_statement()?;
    }

    Ok(StepIndex {
        bytes,
        schemas,
        entries,
        by_id,
        types,
        type_lookup,
        by_type,
        duplicates,
    })
}

fn collect_schemas(params: &[Value], out: &mut Vec<String>) {
    for param in params {
        match param {
            Value::List(values) => collect_schemas(values, out),
            Value::String(schema) => out.push(schema.clone()),
            _ => {}
        }
    }
}
