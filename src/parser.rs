#![allow(dead_code)]
use std::collections::VecDeque;
use std::iter::Peekable;
use lexer::Lexer;
use token::{Token, TokenType};
use base64::Base64;
use base64;
use crc24;
use std::error;
use std::fmt;


const BASE64_LINE_LENGTH: usize = 76;


#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MessageType {
    PGPMessage,
    PGPPublicKeyBlock,
    PGPPrivateKeyBlock,
    PGPSignature,
    PGPMessagePartXofY(usize, usize),
    PGPMessagePartX(usize)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HeaderType {
    Version,
    Comment,
    MessageID,
    Hash,
    Charset,
    OtherHeader(String)
}

fn token_type_to_header_type(token_type: TokenType) -> HeaderType {
    match token_type {
        TokenType::Version   => HeaderType::Version,
        TokenType::Comment   => HeaderType::Comment,
        TokenType::MessageID => HeaderType::MessageID,
        TokenType::Hash      => HeaderType::Hash,
        TokenType::Charset   => HeaderType::Charset,
        _                    => HeaderType::OtherHeader(String::new())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Header {
    header_type: MessageType,
    header_block: Vec<(HeaderType, String)>
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Body {
    body: Base64,
    checksum: crc24::Crc24
}

pub struct ArmorMessage {
    header_type: MessageType,
    header_block: Vec<(HeaderType, String)>,
    body: String,
    checksum: String
}

impl ArmorMessage {
    pub fn new(header_type: MessageType,
           header_block: Vec<(HeaderType, String)>,
           body: String,
           checksum: String) -> ArmorMessage
    {
        ArmorMessage {
            header_type: header_type,
            header_block: header_block,
            body: body,
            checksum: checksum
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ParseError {
    CorruptHeader,
    InvalidHeaderLine,
    CorruptBody,
    EndOfFile,
    ParseError,
}

pub type ParseResult<T> = Result<T, ParseError>;

impl ParseError {
    fn eof<T>() -> ParseResult<T> {
        Err(ParseError::EndOfFile)
    }

    fn parse_error<T>() -> ParseResult<T> {
        Err(ParseError::ParseError)
    }

    fn corrupt_header<T>() -> ParseResult<T> {
        Err(ParseError::CorruptHeader)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ParseError::CorruptHeader => write!(f, "Corrupt header"),
            ParseError::InvalidHeaderLine => write!(f, "Invalid header line."),
            ParseError::CorruptBody => write!(f, "Corrupt Base64 data."),
            ParseError::EndOfFile => write!(f, "Reached end of armored data."),
            ParseError::ParseError => write!(f, "Parser error.")
        }
    }
}

impl error::Error for ParseError {
    fn description(&self) -> &str {
        match *self {
            ParseError::CorruptHeader => "The header data is corrupted.",
            ParseError::InvalidHeaderLine => "A header line contains invalid data.",
            ParseError::CorruptBody => "The Base 64 payload of the armor message was corrupted.",
            ParseError::EndOfFile => "There is no more data available.",
            ParseError::ParseError => "A general parsing error."
        }
    }
}

pub struct Parser<S> where S: Iterator<Item=char> {
    input:  Peekable<Lexer<S>>,
    lookahead: VecDeque<Token>,
    markers: Vec<usize>,
    offset: usize
}

impl<S> Parser<S> where S: Iterator<Item=char> {
    pub fn new(input: Lexer<S>) -> Parser<S> {
        Parser {
            input:     input.peekable(),
            lookahead: VecDeque::with_capacity(20),
            markers:   Vec::new(),
            offset:    0
        }
    }

    fn peek_token(&mut self) -> Option<Token> {
        if self.lookahead.is_empty() {
            self.offset = 0;
            let next_token = self.input.next();
            if next_token.is_some() {
                self.lookahead.push_back(next_token.clone().unwrap());
                Some(next_token.unwrap())
            } else {
                None
            }
        } else {
            self.sync();
            Some(self.lookahead[self.offset].clone())
        }
    }

    fn peek_token_or_eof<F,T>(&mut self, f: F) -> ParseResult<T>
        where F: Fn(&mut Self, Token) -> ParseResult<T>
    {
        match self.peek_token() {
            Some(token) => f(self, token),
            None => Err(ParseError::EndOfFile)
        }
    }

    fn read_token(&mut self) {
        self.offset += 1;
    }

    fn read_tokens(&mut self, amount: usize) {
        self.offset += amount;
    }

    fn sync(&mut self) {
        if self.offset > self.lookahead.len()-1 {
            let n = self.offset - (self.lookahead.len()-1);
            self.fill(n);
        }
    }

    fn fill(&mut self, amount: usize) {
        for _ in 0..amount {
            match self.input.next() {
                Some(token) => {
                    self.lookahead.push_back(token);
                }
                None => break
            }
        }
    }

    fn mark(&mut self) {
        self.markers.push(self.offset);
    }

    fn consume(&mut self) {
        for _ in 0..self.offset {
            self.lookahead.pop_front();
        }
        self.markers.clear();
        self.offset = 0;
    }

    fn backtrack(&mut self) {
        if self.markers.is_empty() {
            self.offset = 0;
        } else {
            self.offset = self.markers.pop().unwrap();
        }
    }

    fn backtrack_with_error<T>(&mut self, result: ParseResult<T>) -> ParseResult<T> {
        if result.is_err() {
            self.backtrack();
        }
        result
    }

    fn try_or_backtrack<F,T>(&mut self, f: F) -> ParseResult<T>
        where F: Fn(&mut Self) -> ParseResult<T>
    {
        match f(self) {
            Ok(res) => Ok(res),
            Err(e)  => self.backtrack_with_error(Err(e))
        }
    }

    fn parse_number(&mut self) -> ParseResult<usize> {
        self.mark();
        let mut result = String::new();
        while let Some(token) = self.peek_token() {
            match token.token_type() {
                TokenType::Digit => {
                    self.read_token();
                    result.push_str(token.as_str());
                }
                _ => break
            }
        }

        if !result.is_empty() {
            let parse_result = result.parse::<usize>().unwrap();
            Ok(parse_result)
        } else if self.peek_token().is_none() {
            Err(ParseError::EndOfFile)
        } else {
            self.backtrack();
            Err(ParseError::ParseError)
        }
    }

    fn read_token_or_else(&mut self, token_type: TokenType, err: ParseError) -> ParseResult<Token> {
        let result = try!(self.peek_token_or_eof(|_, token| {
            if token.has_token_type(token_type) {
                Ok(token)
            } else {
                Err(err)
            }
        }));

        self.read_token();
        Ok(result)
    }

    fn parse_token_lazy<T, F, E>(&mut self, token_type: TokenType, f: F, e: E) -> ParseResult<T>
        where F: Fn(TokenType) -> T,
              E: Fn() -> ParseError
    {
        self.mark();
        self.peek_token_or_eof(|parser, token| {
            if token.has_token_type(token_type) {
                parser.read_token();
                Ok(f(token_type))
            } else {
                parser.backtrack_with_error(Err(e()))
            }
        })
    }

    fn parse_part_x(&mut self) -> ParseResult<usize> {
        self.mark();
        let result = try!(self.try_or_backtrack(Self::parse_number));

        self.peek_token_or_eof(|parser, token| {
            match token.token_type() {
                TokenType::FiveDashes => Ok(result),
                _ => parser.backtrack_with_error(Err(ParseError::CorruptHeader))
            }
        })
    }

    fn parse_part_x_div_y(&mut self) -> ParseResult<(usize, usize)> {
        self.mark();
        let num_x = try!(self.try_or_backtrack(Self::parse_number));

        match self.peek_token() {
            Some(token) => {
                match token.token_type() {
                    TokenType::ForwardSlash => {
                        self.read_token();
                    }
                    _ => return self.backtrack_with_error(Err(ParseError::CorruptHeader))
                }
            }
            None => return self.backtrack_with_error(Err(ParseError::EndOfFile))
        }

        let num_y = try!(self.try_or_backtrack(Self::parse_number));

        Ok((num_x, num_y))
    }

    fn parse_pgp_message_part(&mut self) -> ParseResult<MessageType> {
        self.mark();
        self.peek_token_or_eof(|parser, token| {
            match token.token_type() {
                TokenType::PGPMessagePart => {
                    parser.read_token();
                    match parser.parse_part_x_div_y() {
                        Ok((x,y)) => {
                            return Ok(MessageType::PGPMessagePartXofY(x,y))
                        }
                        Err(_)    => parser.backtrack()
                    }
                    match parser.parse_part_x() {
                        Ok(x)  => {
                            Ok(MessageType::PGPMessagePartX(x))
                        }
                        Err(_) => parser.backtrack_with_error(Err(ParseError::CorruptHeader))
                    }
                }
                _ => parser.backtrack_with_error(Err(ParseError::CorruptHeader))
            }
        })
    }

    fn parse_pgp_message(&mut self) -> ParseResult<MessageType> {
        self.parse_token_lazy(TokenType::PGPMessage,
            |_| { MessageType::PGPMessage },
            || { ParseError::CorruptHeader }
        )
    }

    fn parse_pgp_publickey_block(&mut self) -> ParseResult<MessageType> {
        self.parse_token_lazy(TokenType::PGPPublicKeyBlock,
            |_| { MessageType::PGPPublicKeyBlock },
            || { ParseError::CorruptHeader }
        )
    }

    fn parse_pgp_privatekey_block(&mut self) -> ParseResult<MessageType> {
        self.parse_token_lazy(TokenType::PGPPrivateKeyBlock,
            |_| { MessageType::PGPPrivateKeyBlock },
            || { ParseError::CorruptHeader }
        )
    }

    fn parse_pgp_signature(&mut self) -> ParseResult<MessageType> {
        self.parse_token_lazy(TokenType::PGPSignature,
            |_| { MessageType::PGPSignature },
            || { ParseError::CorruptHeader }
        )
    }

    fn parse_header_tail_line(&mut self, token_type: TokenType) -> ParseResult<MessageType> {
        try!(self.read_token_or_else(TokenType::FiveDashes, ParseError::CorruptHeader));
        try!(self.read_token_or_else(token_type, ParseError::CorruptHeader));

        let message_type = try!(self.peek_token_or_eof(|parser, token| {
            match token.token_type() {
                TokenType::PGPMessagePart     => parser.parse_pgp_message_part(),
                TokenType::PGPMessage         => parser.parse_pgp_message(),
                TokenType::PGPPublicKeyBlock  => parser.parse_pgp_publickey_block(),
                TokenType::PGPPrivateKeyBlock => parser.parse_pgp_privatekey_block(),
                TokenType::PGPSignature       => parser.parse_pgp_signature(),
                _ => return Err(ParseError::CorruptHeader)
            }
        }));

        try!(self.read_token_or_else(TokenType::FiveDashes, ParseError::CorruptHeader));

        self.consume();
        Ok(message_type)
    }

    fn parse_header_line(&mut self) -> ParseResult<MessageType> {
        self.parse_header_tail_line(TokenType::Begin)
    }

    fn parse_tail_line(&mut self) -> ParseResult<MessageType> {
        self.parse_header_tail_line(TokenType::End)
    }

    fn parse_header_text(&mut self) -> ParseResult<String> {
        let mut result = String::new();
        loop {
            match self.peek_token() {
                Some(token) => {
                    match token.token_type() {
                        TokenType::Version
                            | TokenType::Comment
                            | TokenType::MessageID
                            | TokenType::Hash
                            | TokenType::Charset
                            | TokenType::BlankLine => {
                                break;
                        }
                        _ => {
                            result.push_str(token.as_str());
                            self.read_token();
                        }
                    }
                }
                None => return Err(ParseError::EndOfFile)
            }
        }

        Ok(result)
    }

    fn skip_whitespace(&mut self) {
        while let Some(token) = self.peek_token() {
            match token.token_type() {
                TokenType::WhiteSpace => {
                    self.read_token();
                }
                _ => break
            }
        }
    }

    fn parse_headerkv(&mut self) -> ParseResult<(HeaderType, String)> {
        let header_type = try!(self.peek_token_or_eof(|parser, token| {
            match token.token_type() {
                tt @ TokenType::Version
                    | tt @ TokenType::Comment
                    | tt @ TokenType::MessageID
                    | tt @ TokenType::Hash
                    | tt @ TokenType::Charset => {
                        parser.read_token();
                        parser.skip_whitespace();
                        Ok(token_type_to_header_type(tt))
                }
                _ => return Err(ParseError::InvalidHeaderLine)
            }
        }));

        try!(self.peek_token_or_eof(|parser, token| {
            match token.token_type() {
                TokenType::ColonSpace => {
                    parser.read_token();
                    parser.skip_whitespace();
                    Ok(())
                }
                _ => return Err(ParseError::InvalidHeaderLine)
            }
        }));
        let header_text = try!(self.peek_token_or_eof(|parser, _| parser.parse_header_text()));

        self.consume();
        Ok((header_type, header_text))
    }

    fn parse_header_block(&mut self) -> ParseResult<Vec<(HeaderType, String)>> {
        let mut result = Vec::new();
        loop {
            match self.peek_token() {
                Some(token) => {
                    match token.token_type() {
                        TokenType::Version
                        | TokenType::Comment
                        | TokenType::MessageID
                        | TokenType::Hash
                        | TokenType::Charset => {
                            try!(self.parse_headerkv()
                                     .map(|(key, val)| { result.push((key, val)); }));
                        }
                        TokenType::BlankLine => {
                            self.read_token();
                            break;
                        }
                        _ => {
                            return Err(ParseError::CorruptHeader)
                        }
                    }
                }
                None => {
                    return Err(ParseError::EndOfFile)
                }
            }
        }

        self.consume();
        Ok(result)
    }

    fn parse_header(&mut self) -> ParseResult<Header> {
        let header_type: MessageType = try!(self.parse_header_line());
        self.skip_whitespace();
        let header_block: Vec<(HeaderType, String)> = try!(self.parse_header_block());

        let header = Header {
            header_type: header_type,
            header_block: header_block
        };

        self.consume();
        Ok(header)
    }

    fn parse_tail(&mut self) -> ParseResult<MessageType> {
        self.parse_tail_line()
    }

    fn parse_body_line(&mut self) -> ParseResult<String> {
        self.mark();
        let mut line = String::new();
        let mut i = 0;
        while i < BASE64_LINE_LENGTH {
            match self.peek_token() {
                Some(token) => {
                    match token.token_type() {
                        TokenType::NewLine => {
                            break;
                        }
                        TokenType::Pad => {
                            // Parse out the padding to newline
                            match self.parse_padding() {
                                Ok(amount) => {
                                    // We can consume the padding.
                                    if i + amount < BASE64_LINE_LENGTH {
                                        self.read_tokens(amount);
                                        break;
                                    } else {
                                        return self.backtrack_with_error(Err(ParseError::CorruptBody));
                                    }
                                }
                                Err(e) => return self.backtrack_with_error(Err(e))
                            }
                        }
                        _ => {
                            let slice = token.as_str();
                            if i + slice.len() < BASE64_LINE_LENGTH {
                                line.push_str(slice);
                                i += 1;
                                self.read_token();
                            } else {
                                return self.backtrack_with_error(Err(ParseError::CorruptBody));
                            }
                        }
                    }
                }
                None => return Err(ParseError::EndOfFile)
            }
        }
        // We have hit the maximum length a line of ascii armor text can be.
        // If the next character is not a newline character, the armor is corrupted.
        // An base64 line of the body must be at most 76 characters, not including a newline.
        match self.peek_token() {
            Some(token) => {
                match token.token_type() {
                    TokenType::NewLine => {
                        self.read_token();
                    }
                    _ => {
                        return self.backtrack_with_error(Err(ParseError::CorruptBody));
                    }
                }
            }
            None => return Err(ParseError::EndOfFile)
        }

        self.consume();
        Ok(line)
    }

    fn parse_padding(&mut self) -> ParseResult<usize> {
        let mut count = 0;
        loop {
            match self.peek_token() {
                Some(token) => {
                    match token.token_type() {
                        TokenType::Pad => {
                            self.read_token();
                            count += 1;
                        }
                        _ => break
                    }
                }
                None => return Err(ParseError::EndOfFile)
            }
        }

        Ok(count)
    }

    fn parse_body(&mut self) -> ParseResult<String> {
        self.mark();
        let mut string = String::new();
        loop {
            match self.parse_body_line() {
                Ok(other_string) => {
                    string.push_str(other_string.as_str());
                    match self.peek_token() {
                        Some(token) => {
                            match token.token_type() {
                                TokenType::Pad => {
                                    // We are at the end of the base 64 data.
                                    break;
                                }
                                TokenType::FiveDashes => {
                                    // The padding line is missing.
                                    return self.backtrack_with_error(Err(ParseError::CorruptBody));
                                }
                                _ => {
                                    continue;
                                }
                            }
                        }
                        None => return Err(ParseError::EndOfFile)
                    }
                }
                Err(e) => return Err(e)
            }
        }

        self.consume();
        Ok(string)
    }

    fn parse_checksum(&mut self) -> ParseResult<String> {
        self.mark();

        match self.peek_token() {
            Some(token) => {
                match token.token_type() {
                    TokenType::Pad => {
                        self.read_token();
                    }
                    _ => return Err(ParseError::CorruptBody)
                }
            }
            None => return Err(ParseError::EndOfFile)
        }

        let mut checksum = String::new();
        let mut i = 0;
        while i < 4 {
            match self.peek_token() {
                Some(token) => {
                    if base64::is_base64(token.as_bytes()) && (i + token.as_bytes().len() < 4) {
                        checksum.push_str(token.as_str());
                        i += token.as_bytes().len();
                    } else {
                        return Err(ParseError::CorruptBody)
                    }
                }
                None => return Err(ParseError::EndOfFile)
            }
        }

        match self.peek_token() {
            Some(token) => {
                match token.token_type() {
                    TokenType::NewLine => {
                        self.read_token();
                    }
                    _ => return Err(ParseError::CorruptBody)
                }
            }
            None => return Err(ParseError::EndOfFile)
        }

        self.consume();
        Ok(checksum)
    }

    pub fn parse(&mut self) -> ParseResult<ArmorMessage> {
        let header   = try!(self.parse_header());
        let body     = try!(self.parse_body());
        let checksum = try!(self.parse_checksum());
        let tail     = try!(self.parse_tail());

        if header.header_type == tail {
            Ok(ArmorMessage::new(header.header_type, header.header_block, body, checksum))
        } else {
            Err(ParseError::ParseError)
        }
    }
}


#[cfg(test)]
mod tests {
    use lexer::Lexer;
    use super::{Parser, HeaderType, MessageType, Header};


    struct HeaderLineTest {
        header_line: String,
        header_type: MessageType
    }

    impl HeaderLineTest {
        fn new(header_line: &str, header_type: MessageType) -> HeaderLineTest {
            HeaderLineTest {
                header_line: String::from(header_line),
                header_type: header_type
            }
        }
    }

    fn run_header_line_test(test: &HeaderLineTest) {
        let lexer  = Lexer::new(test.header_line.chars());
        let mut parser = Parser::new(lexer);
        let result = parser.parse_header_line();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), test.header_type);
    }

    fn run_tail_line_test(test: &HeaderLineTest) {
        let lexer  = Lexer::new(test.header_line.chars());
        let mut parser = Parser::new(lexer);
        let result = parser.parse_tail_line();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), test.header_type);
    }

    #[test]
    fn test_parse_pgp_message_header_line() {
        let test = HeaderLineTest::new("-----BEGIN PGP MESSAGE-----\n\n", MessageType::PGPMessage);
        run_header_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_message_tail_line() {
        let test = HeaderLineTest::new("-----END PGP MESSAGE-----\n\n", MessageType::PGPMessage);
        run_tail_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_signature_header_line() {
        let test = HeaderLineTest::new("-----BEGIN PGP SIGNATURE-----\n\n", MessageType::PGPSignature);
        run_header_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_signature_tail_line() {
        let test = HeaderLineTest::new("-----END PGP SIGNATURE-----\n\n", MessageType::PGPSignature);
        run_tail_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_publickey_block_header_line() {
        let test = HeaderLineTest::new("-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n", MessageType::PGPPublicKeyBlock);
        run_header_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_publickey_block_tail_line() {
        let test = HeaderLineTest::new("-----END PGP PUBLIC KEY BLOCK-----\n\n", MessageType::PGPPublicKeyBlock);
        run_tail_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_privatekey_block_header_line() {
        let test = HeaderLineTest::new("-----BEGIN PGP PRIVATE KEY BLOCK-----\n\n", MessageType::PGPPrivateKeyBlock);
        run_header_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_privatekey_block_tail_line() {
        let test = HeaderLineTest::new("-----END PGP PRIVATE KEY BLOCK-----\n\n", MessageType::PGPPrivateKeyBlock);
        run_tail_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_message_part_xofy_header_line() {
        let test = HeaderLineTest::new("-----BEGIN PGP MESSAGE, PART 1/13-----\n\n", MessageType::PGPMessagePartXofY(1,13));
        run_header_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_message_part_xofy_tail_line() {
        let test = HeaderLineTest::new("-----END PGP MESSAGE, PART 1/13-----\n\n", MessageType::PGPMessagePartXofY(1,13));
        run_tail_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_message_parts_indefinite_header_line() {
        let test = HeaderLineTest::new("-----BEGIN PGP MESSAGE, PART 1-----\n\n", MessageType::PGPMessagePartX(1));
        run_header_line_test(&test);
    }

    #[test]
    fn test_parse_pgp_message_parts_indefinite_tail_line() {
        let test = HeaderLineTest::new("-----END PGP MESSAGE, PART 1-----\n\n", MessageType::PGPMessagePartX(1));
        run_tail_line_test(&test);
    }

    struct HeaderTestCase {
        text: String,
        header: Header
    }

    impl HeaderTestCase {
        fn new(text: &str, header: Header) -> HeaderTestCase {
            HeaderTestCase {
                text: String::from(text),
                header: header
            }
        }
    }

    struct HeaderTest {
        data: Vec<HeaderTestCase>
    }

    fn header_test_cases() -> HeaderTest {
        HeaderTest {
            data: vec![
                HeaderTestCase {
                    text: String::from(
                        "-----BEGIN PGP MESSAGE-----\
                        Version: OpenPrivacy 0.99\n\
                        Comment: Foo Bar Baz\n\
                                \n\
                        yDgBO22WxBHv7O8X7O/jygAEzol56iUKiXmV+XmpCtmpqQUKiQrFqclFqUDBovzS\n\
                        vBSFjNSiVHsuAA==\n\
                        =njUN\n\
                        -----END PGP MESSAGE-----
                        "),
                    header: Header {
                        header_type:  MessageType::PGPMessage,
                        header_block: vec![
                            (HeaderType::Version, String::from("OpenPrivacy 0.99")),
                            (HeaderType::Comment, String::from("Foo Bar Baz"))
                        ]
                    }
                }
            ]
        }
    }

    fn run_header_tests(tests: &HeaderTest) {
        for test_case in tests.data.iter() {
            let lexer = Lexer::new(test_case.text.chars());
            let mut parser = Parser::new(lexer);
            let result = parser.parse_header().unwrap();
            assert_eq!(result.header_type, test_case.header.header_type);
        }
    }

    #[test]
    fn test_header() {
        run_header_tests(&header_test_cases());
    }

}
